# Issue #1016 Atomic CI Verifier Rewrite Design

## Decision and status

Issue #1016 will replace the central CI workflow-hygiene verifier atomically.
An approved external trust root must exist first; the permanent repository seam then lands as semantics-preserving infrastructure, and the atomic replacement is the first covered enforcement change after freeze.
The final head contains no legacy implementation, comparison path, or compatibility implementation.

This design is approved by direct user ruling.
The issue body still prescribes an incremental, no-big-bang approach, so its scope must be explicitly reconciled before publication or implementation.
The reconciliation must state that the direct ruling selects atomic cutover; it may not silently reinterpret the issue body.

## Authority and baseline

Authority descends from the direct ruling, `AGENTS.md`, the reconciled issue body, this design, and the eventual implementation plan.
This design was written from exact base `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`.
Merged Program-A PRs #1364, #1365, #1368, and #1369 produced a combined repository delta of +4/-126, removed 118 net Python lines, and define the accepted semantic freeze boundary.
Small owner- or dependency-blocked packets defer beyond #1016 and may not enter between the one permanent precursor and atomic replacement.

The measured `scripts/` Python baseline is 113 files and 143,819 lines.
The legacy atomic cluster is 26,667 lines:

- `scripts/verify_ci_workflow_hygiene.py`: 11,426.
- `scripts/test_verify_ci_workflow_hygiene.py`: 12,966.
- `scripts/ci_workflow_hygiene_test_helpers.py`: 2,149.
- `scripts/test_rust_verification_decoupling.py`: 126.

Attributable repository replacement code and tests across all languages must not exceed 13,333 lines.
Non-executable corpus/manifest data is excluded and reported separately; non-Python repository code is included and subtotaled, while external-App code remains separate but visible.
This is a one-time architecture budget, not a new continuing line-count fence.

## Scope and non-goals

Issue #1016 owns only the central verifier architecture:

- Repository integration/evidence for a separately approved candidate-immutable trust root, exact authority selection, and policy-base validation.
- Surface classification, restricted parsers, typed snapshots, and semantic dispatch.
- Stable rule ownership, structured findings, and corpus-driven weakening defense.
- The stable command-line facade, focused tests, and atomic legacy deletion.
- Direct and dynamic caller migration required to leave one path.

It may add only the bootstrap needed to make those components hermetic and narrow adapters to retained domain owners.
It does not authorize or own creation, funding, installation, or operation of the external App control plane.
It does not change runtime topology or runtime policy.
It does not refactor or assume ownership of `ci_provenance`, `merge_readiness`, `coverage_enforcer`, merge-queue execution, `rust_verification`, storage, AI review, `run_fences`, or operator tools.
Those names appear only as integration boundaries and later separately issue-owned programs.

## Governing invariants

1. The candidate cannot control whether the required check runs, its identity, or its success conversion.
2. Candidate paths, conditions, skipped jobs, or continuation settings cannot disable the check.
3. The external App launcher is identified by immutable launcher SHA and artifact digest; manifest, protocol, corpus, applicability, and runner come from exact `policy_base_sha`.
4. Repository inputs and candidate implementation come from the exact pull-request head.
5. Head-controlled applicability is forbidden.
6. Missing, malformed, timed-out, crashed, incomplete, or unknown outcomes fail closed.
7. Every stable semantic rule has exactly one owner and one typed input contract.
8. Every retained or re-homed rule has a clean case and at least one falsifying mutation.
9. Findings are structured, deterministic, complete, and attributable to a stable rule ID.
10. Each governed input type has one restricted parser and no raw fallback.
11. Runtime limits and lifecycle-coupled values remain grouped in TOML configuration.
12. The final head contains one enforcement path and no compatibility exports.

## Ordered sequence and freeze

### 1. Close the accepted wave

Treat the four merged Program-A PRs as the last accepted semantic wave.
Do not use stale branches or superseded plans as evidence against fresh `main`; defer blocked packets.

### 2. Regenerate from fresh main

From clean fresh `main`, regenerate the caller closure, stable-rule manifest, mutation corpus, timing evidence, and peak-memory evidence.
Record the exact source SHA and commands; reject dirty state, stale `main`, or irreproducible artifacts.

### 3. Land the one all-assets permanent precursor alone

First establish the separately approved external App, check binding, ruleset, and operating budget.
Exactly one repository PR then installs every permanent asset: revision resolver, manifest, protocol, materializer/runner, corpus, and parser bootstrap.
Its head artifacts are untrusted installation targets; the legacy verifier never speaks or translates the protocol.
The event-specific external authorization and App-owned conformance proof are defined below, and this PR queues alone.

### 4. Freeze

After merge, revoke the one-use envelope, run a closed conformance canary from protected state, and freeze.
The Program-A wave remains the semantic boundary; the precursor merely makes it independently enforceable.
All covered manifest, corpus, applicability, expectation, and verifier changes are prohibited until #1016 merges.
Any intervening covered change invalidates the freeze and requires fresh closure, corpus, timing, memory, canary, and freeze evidence.

### 5. Replace atomically

The #1016 replacement is the first subsequent covered change and the first real repository candidate to speak the permanent protocol.
One head changes implementation, focused tests, suite registries, fingerprints, pins, and all applicable callers, while deleting the entire legacy path.
There are no extraction, dual-run, or cleanup PRs.

## Protected launch and base/head split

The primary trust root is a dedicated external private GitHub App operating outside this repository.
It publishes required check `trusted-ci-verifier`, bound by ruleset to the App's distinct integration ID, and supports both `pull_request` and `merge_group`.
Its tokens, signing keys, launcher artifact, and control plane remain outside candidate code; candidate execution is network-disabled with bounded resources.
The App prerequisite is a separately approved control plane with an explicit code/maintenance/hosting budget; this design does not authorize creating, funding, installing, or operating it.

Existing GitHub Actions checks are spoofable under shared integration ID `15368`.
Candidate-controlled Actions, `pull_request_target`, and native-review ceremony are rejected trust roots.
An organization required-workflow binding is fallback only and requires GitHub Enterprise plus repository transfer into the governing organization.
#1016 is blocked before precursor implementation unless the private App or that separately approved fallback exists and proves immutable identity.

Each event's signed authority record binds repository ID; App integration and installation IDs; check context `trusted-ci-verifier`; ruleset ID, version, and digest; launcher SHA and artifact digest; `policy_base_sha`; `head_sha`; event kind and delivery ID; and, for merge groups, identity plus constituent heads.
`policy_base_sha` is exact PR `base.sha` or merge-group `base_sha`; merge base is diff-only and never policy authority.
The App re-queries every field immediately before terminal success; stale/moved identity, object, ancestry, tree, parent, constituent, or digest mismatch fails closed.

The App owns a separately signed installation-verifier/conformance bundle with inputs and expectations.
Precursor-head runner, manifest, protocol, and corpus are untrusted target artifacts: the App treats them as inert for digest/schema checks and may execute the head runner only sandboxed as an untrusted subject against App-owned conformance cases.
Candidate self-validation is never accepted; conformance covers clean, always-pass/fail, wrong/duplicate ID, malformed/incomplete output, crash, and timeout.

For the precursor PR, the App mints one signed, expiring, event-specific envelope bound to the complete authority record, protected `main`, reviewed path allowlist, and content digests; the resulting `trusted-ci-verifier` check is valid authority for that exact PR head.
Every Mergify proof PR or `merge_group` head requires its own fresh non-reusable result under the same required context after API verification of exact base, approved precursor head, parent/tree derivation, constituent heads, and absence of extra covered changes; no result carries across SHAs or events and no second context is introduced.
The exact synthetic merge-group head receives a separate non-reusable merge-authority result; no generic renewal or derived-head wildcard exists.

After merge, the App verifies that the exact reviewed bytes are now at protected `policy_base_sha` before promoting them to policy authority, then records an external monotonic tombstone keyed to precursor SHA once it is an ancestor of protected `main`.
Every later bootstrap envelope is rejected regardless of signer; the pinned post-freeze App artifact has bootstrap acceptance removed or irreversibly disabled.
Replay, reissue, datastore rollback, and old-App-artifact restoration must all fail before the closed canary and freeze.
The legacy verifier never emits or translates the protocol; #1016 remains its first real repository candidate.

## Corpus evolution and delayed weakening defense

During the precursor-to-#1016 freeze, no covered corpus or manifest change is allowed, including additions.
After cutover, strengthening additions must land in PRs isolated from candidate behavior changes and become selectable only after joining protected policy-base state.
Deletion, narrowing, applicability or expectation change, and rename all use two separately governed retirement stages:

1. An owner-approved retirement record lands while the old case remains active, naming the stable rule, replacement owner, semantic-superset argument, and falsifying evidence.
2. A later isolated PR applies the change only after the replacement passes exact-head CI from protected state; it contains no candidate behavior change or unrelated enforcement change.

Rule renames use a base-owned, one-to-one, time-bounded identity migration and follow the same retirement process.
The validator rejects missing cases, unexpected applicability changes, and incomplete finding sets.

## Target components, interfaces, and dependency direction

### Protected launcher and base runner

The App may only authenticate events, resolve/revalidate the authority record, transport materialized bundles into a sandbox, validate the protocol envelope, and publish the check.
It performs no parsing, semantic-rule execution, corpus interpretation, or hidden central-verifier logic; auditable source-to-artifact provenance binds its pinned launcher.
The `policy_base_sha` runner schedules internal cases and a policy-base materializer combines post-head bytes with CSPRNG-selected semantic/metamorphic variants into immutable document bundles, recording the private seed for authorized replay.
Repository/head bytes and mutated case bytes remain distinct provenance fields through parsing and reporting.
Every run evaluates the actual-head clean tree plus multiple runtime variants; corpus semantics and stable rule IDs remain public, while case IDs and expectations do not enter the sandbox.

### Surface manifest and protocol validator

A `policy_base_sha` TOML owner maps governed paths to explicit classifications and stable rule IDs, and groups protocol version, resource limits, parser bootstrap identity, and corpus locations by lifecycle.
Unknown files inside governed surfaces are classification failures, never silently ignored.
The validator checks schema, identity, completeness, ordering, uniqueness, and termination without inferring findings from text.

### Candidate controller and classifier

The controller receives only the immutable materialized document bundle, never repository paths or a repository checkout, then classifies, parses, snapshots, dispatches, and emits the protocol.
Syntax classification maps each document to exactly one native class: GitHub workflow YAML, GitHub action YAML, TOML, Just, shell, or Mergify/Psych.
The separate semantic-owner map assigns typed snapshots to stable rules or retained owners.
The real generic NO DUAL PATHS rule lives in classification and rejects competing active owners or execution paths.

### Restricted parsers and typed snapshots

The external sandbox mount proves byte-access restriction; a policy-base counting-loader harness separately records one byte load and one parser/native-owner invocation per selected document.
These internal counters are evidence under test, not candidate-trusted authority; architecture proof also requires native/external review, without a new syscall broker.
Static dependency/import fences prevent raw I/O or parser imports outside the snapshot owner, and retained native owners accept only stdin or immutable typed inputs.
`shell_dataflow_analysis` is the sole shell semantic parser; Just uses one native dump; Mergify is its own Ruby/Psych syntax class.
Snapshots expose only normalized jobs, events, permissions, steps, commands, dependencies, configuration facts, absence/type states, and source spans.

### Stable semantic registry and retained adapters

The registry maps every stable rule ID to one owner and typed contract, with total dispatch for the manifested set.
Unknown, duplicate, generic, or unowned IDs are failures.
Narrow adapters call retained domain owners with immutable typed models and translate findings; they provide no bulk re-exports, symbol bags, alternate entrypoints, or parser access.

### Reporter and direction

The base reporter renders only validated structured findings; human text is not candidate authority.
Dependencies flow as follows:

`external App launcher -> policy-base runner -> manifest/materializer/corpus + isolated candidate -> protocol validator -> reporter`

`candidate controller -> classifier -> restricted parser -> typed snapshot -> semantic registry -> narrow adapter`

Parsers do not import rules; rules do not import launch, reporting, corpus, or raw parsers; adapters do not re-export domain internals.

## Stable protocol and fail-closed behavior

The candidate emits one canonical UTF-8 JSON document and nothing else on the protocol channel.
It binds protocol version, the authority tuple, an opaque one-invocation nonce, immutable case-bundle digest, terminal classification, and a complete ordered finding list.
Candidate findings contain stable `rule_id`, opaque document token, and source `span` or typed `detail`; base validation resolves the token to normalized repository `path` for the final structured finding.
Nonce/digest binding prevents replay but cannot prove semantic honesty; adversarial review and variants inspect baseline-hash, case/content, and digest-specific branching.

The base validator rejects:

- Always-pass and always-fail behavior exposed by mixed clean/mutation cases.
- Generic, wrong, duplicate, unknown, or unowned rule IDs.
- Missing/wrong nonces, bundle digests, or SHAs, replayed/duplicate results, and unselected invocations.
- Malformed JSON, extra output, unknown fields, wrong types, or unsupported versions.
- Swallowed parser errors or parser errors disguised as semantic findings.
- Incomplete/unexpected findings, unstable order, or lossy path normalization.
- Missing terminal output, nonzero exit, crash, timeout, signal, or resource exhaustion.

Every rejection fails the required check.
Runner classifications remain separate from candidate findings so a crash cannot masquerade as policy output.

## Restricted parser and hermetic bootstrap

GitHub workflow/action YAML uses PyYAML 6.0.2 through one GitHub-specific safe loader.
A `policy_base_sha`-owned complete hash lock feeds a content-addressed isolated CPython 3.12 environment.
Policy-base TOML declares the exact supported interpreter/platform matrix: CPython 3.12 on local macOS arm64 and CI manylinux x86-64/aarch64, with exact wheel tags; no other host is supported.
The complete lock's wheel hashes must equal the matrix-derived exact set; an unsupported host or missing/extra wheel hash is `bootstrap_failure`.
The single public recipe `just provision-ci-verifier-python` scrubs `PYTHONPATH`, `PYTHONHOME`, and pip configuration, then performs an atomic lock/install with `pip --require-hashes --only-binary=:all: --no-deps` and no ambient-package fallback.
Both `ci-lint-workflow` and existing source-fence dependency provisioning call that recipe without changing job topology or policy.
A static exact-set closure test proves local and remote CI lint, full/static source-fence, and direct verifier entrypoints all traverse that recipe.
Raw PyYAML installation, alternate locks, ambient imports, or direct unprovisioned invocation fail the closure test.

A fresh network-backed bootstrap must succeed; a validated warm-cache bootstrap must succeed offline.
Cache miss while offline, network failure, hash mismatch, corrupt artifact, incomplete lock, or non-atomic install maps to `bootstrap_failure`.
The content address binds interpreter identity, lock bytes, artifact hashes, and bootstrap recipe version.
Every cache reuse requires an atomic ready manifest bound to lock, bootstrap, interpreter, and platform digests, plus verification of installed distribution/version, module origin, and environment isolation.
Poisoned cache, ambient wrong version, system-site leakage, interrupted construction, missing/extra files, or any digest mismatch must test as `bootstrap_failure`.

The safe loader keeps key `on` as a string, accepts GitHub-valid scalar, sequence, and mapping trigger values into one typed form, and preserves source marks.
It rejects duplicate keys, anchors, aliases, merge keys, explicit tags, multiple documents, and wrong node types without raw/regex fallback.
TOML uses one standard type-checking adapter; shell, Just, and Mergify use the single native owners defined above.

Vendoring PyYAML is rejected because it creates an untracked fork and patch/update burden.
A `ruamel.yaml` wheelhouse is rejected because it adds an unnecessary parser family and does not reuse the repository's hash-locked dependency provisioning model.

## Atomic cutover and caller migration

`scripts/verify_ci_workflow_hygiene.py` remains a thin stable facade.
It preserves no-argument and `self-authorizing-governance` modes unless every caller of a mode switches in the same head.
It contains argument validation and one delegation, with no rules or compatibility exports.

The atomic head must:

- Migrate all 13 helper consumers to focused fixture owners and delete `scripts/ci_workflow_hygiene_test_helpers.py`.
- Replace and delete `scripts/test_verify_ci_workflow_hygiene.py` with focused rule, parser, protocol, facade, and integration tests.
- First re-home and pass the generic NO DUAL PATHS classification/mutation proof; only then delete `scripts/test_rust_verification_decoupling.py` and historical pins in the same atomic head.
- Remove the old verifier body while retaining only the thin facade.
- Atomically update the suite registry, exact-set tests, both cheap-lane mirrors (`ci/rust-verification.toml` and `crates/backtesting-vertical-slice/ci/rust-verification.toml`), `.github/workflows/ci.yml` fingerprint inputs, and all direct/dynamic callers.
- Delete legacy scanners, digests, relocated-symbol guards, aliases, compatibility exports, and old/new comparisons.

No deleted function alias survives.
Semantic continuity comes from the base corpus and stable rule IDs, not runtime comparison.
The final head has one facade, controller, parser per type, and owner per rule.

### Exact caller-disposition closure

Fresh-main generation produces an exact `policy_base_sha` path/symbol/mode disposition manifest; every entry names destination, disposition, and proof, and the closure verifier rejects any unlisted direct, import, dynamic, symbol, command, or fingerprint edge.

| Current edge | New owner/disposition | Required proof |
| --- | --- | --- |
| Helper loads in `test_nextest_fingerprint.py`, `test_clean_merged_artifacts.py`, `test_workflow_expression_analysis.py`, `test_governance_diff_analysis.py`, `test_cargo_shim.py`, `test_sandbox_safe_push.py` | Same-domain focused fixture owner named per manifest; delete helper mode | Per-path dynamic-load closure and focused owner test |
| Helper loads in `test_rust_verification_decoupling.py`, `test_verify_bolt_v3_boundary_evidence.py`, `test_cargo_command_analysis.py`, `test_merge_queue_preflight.py`, `test_shell_dataflow_analysis.py`, `test_ci_input_sets.py`, and giant `test_verify_ci_workflow_hygiene.py` | Same-domain fixture owner; Rust invariant first re-homed; giant split; all legacy loads/files deleted | Per-path closure, owner tests, and zero helper/giant paths |
| `test_command_understanding` compatibility exports | Import canonical command-analysis owner; delete facade exports | Symbol/import exact set and owner tests |
| `test_ci_storage_tripwire` direct verifier call | Test canonical storage-tripwire owner; registry uses typed adapter | Direct-call scan plus clean/falsifying adapter cases |
| `merge_queue_preflight.py` evidence identity | `trusted-ci-verifier` authority-record evidence owner | Wrong App/launcher/ruleset/event/policy/head mutations fail |
| `verifier_io.py` | Stable facade/protocol command owner | CLI exact set and dynamic invocation test |
| `ci/doc-decoupling-residuals.toml` | Remove resolved legacy residual; retain only independently valid entries | Residual exact set and stale-symbol scan |
| `run_ci_lint_suites.py` registry and exact sets | Focused suite registry owner | Missing, duplicate, and extra-suite mutations fail |
| `ci/rust-verification.toml` and `crates/backtesting-vertical-slice/ci/rust-verification.toml` | New focused labels; remove historical pins | Cross-mirror/registry equality test |
| `.github/workflows/ci.yml` fingerprint/trusted-base calls | Policy-base runner, lock, manifests, corpus, code, tests | Fingerprint/input closure mutations fail |

## Error handling

Boundary classifications are `launch_failure`, `revision_failure`, `bootstrap_failure`, `classification_failure`, `parse_failure`, `protocol_failure`, `candidate_failure`, and `semantic_failure`.
Only a complete clean semantic result passes; unknown classifications fail as protocol failures.
Diagnostics are bounded and never dump environment, credentials, arbitrary binary output, or unbounded source.
Paths are repository-relative and reject traversal, absolute paths, and symlink escape.

## Test and evidence matrix

| Requirement or risk | Required evidence |
| --- | --- |
| Immutable launch | External-App/integration binding survives head workflow deletion; shared-integration spoofing fails |
| Installation authority | App-owned signed conformance rejects self-validation; promoted bytes exactly match reviewed precursor bytes |
| Derived merge authority | PR feedback cannot authorize merge; wrong parent/tree/constituent or extra covered change fails |
| Bootstrap closure | Envelope replay/reissue, datastore rollback, and old-artifact restore fail against monotonic tombstone |
| Exact revisions | Wrong authority-record field, diff base, absent object, movement, or terminal re-query fails |
| Base authority | Head manifest/protocol/corpus/runner edits cannot affect their own check |
| Frozen applicability | Head skip/relabel and same-PR case retirement/weakening fail |
| Private materialization | Candidate sees no case ID/expectation; nonce/digest replay and case-specific branching fail |
| Metamorphic coverage | Runtime semantic variants preserve expected rules and expose hash/content special-casing |
| Stable protocol | Wrong/generic/duplicate/unknown IDs, nonce/digest, and malformed/incomplete output fail |
| Honest terminal result | Mixed clean/falsifying cases reject always-pass and always-fail candidates |
| Parser failures | Each parser's crash, unsupported syntax, wrong type, and partial output fail |
| Parse once | External mount review proves byte scope; counting-loader tests plus import fences prove internal call shape |
| Restricted YAML | Valid trigger scalar/sequence/map pass; duplicate, anchor, alias, merge, tag, multi-doc, and wrong types fail |
| Hermetic bootstrap | Fresh-network and warm-offline pass; miss/network/hash/corruption/lock failures classify correctly |
| Singular rule ownership | Registry exact-set test maps every stable ID to one owner and typed contract |
| Semantic continuity | Every retained/re-homed rule has clean and falsifying cases |
| Generic dual-path rule | Mutations distinguish competing owners/paths from valid independent components |
| Facade contract | No-arg and self-authorizing modes pass end-to-end caller tests |
| Complete migration | Caller closure finds zero helper/legacy imports, loads, symbols, pins, or comparisons |
| Integrated callers | Registry, exact sets, both TOML mirrors, CI fingerprints, and dynamic callers pass |
| Resource budget | Fresh-main and candidate timing/RSS use identical policy-base cases and configuration |
| Final proof | Relevant cheap gates and exact-head required CI pass at the reviewed SHA |

## Delivery and review checkpoints

1. Reconcile the issue body before publication.
2. Confirm separate authorization, budget, deployment, integration/ruleset binding, and event support for the external App or approved fallback.
3. Regenerate clean fresh-main closure, manifest, corpus, timing, and memory artifacts with exact SHA.
4. Assign one implementer to each file set in the single precursor; issue only the PR-feedback and event-derived envelopes defined above.
5. Resolve findings, run internal adversarial review and cheap gates once, then keep draft until exact-head feedback is green.
6. Request external/native review only after green exact-head CI and queue the precursor alone.
7. After merge, verify/promote exact bytes, set the monotonic tombstone, disable bootstrap acceptance, pass the closed canary, and publish freeze evidence.
8. Assign one implementer per atomic-cutover file set and complete all deletion/caller changes in one head.
9. Resolve internal adversarial findings, then run relevant cheap gates once.
10. Keep the PR draft while exact-head feedback is non-green or unresolved.
11. At the final SHA, obtain green exact-head CI, then required native review, last-push approval, and resolved threads.
12. Verify active `main` rules and queue #1016 alone through the repository command.

No external reviewer receives uncommitted, unpushed, non-green, or unresolved work.
Agents report the exact head and detach; the reviewer verifies results at that head.

## Quantitative acceptance

- A `policy_base_sha`-owned path/symbol inventory and deterministic `just ci-verifier-budget-report` command produce the reviewed count and machine-readable attribution report.
- Every attributable repository code/test path across languages is counted in full, including launcher seams, runner/protocol, facade, parsers, registry, adapters, bootstrap, measurement code, and focused tests; symbols prove closure but never discount a path.
- Repository replacement maintained code/tests remain at most 13,333 lines and compare against the 26,667-line cluster without a permanent fence.
- Corpus/manifest exclusions must validate as non-executable and are reported separately; no newly introduced #1016 central policy logic may hide in data, generated output, retained owners, or another language, while existing retained domain policy stays owned in place.
- Non-Python repository code and external-App maintained code are each reported separately; the App also reports maintenance/hosting cost against its separately approved operating budget and cannot disappear from total cost accounting.
- External central semantic verifier, policy, or corpus-interpretation logic is forbidden and would be counted against 13,333 then rejected; the allowed non-semantic App installation verifier/conformance harness is included with launcher/control-plane code and operations in the separate App budget, all bound by source-to-artifact provenance.
- The old verifier body, giant test, helper, historical decoupling test, and pins are absent.
- All 13 current helper consumers have focused owners and no legacy load path.
- Stable-rule owner count equals manifest count; every rule has clean and falsifying cases.
- Each selected case yields exactly one validated terminal result.
- Both cheap-lane TOML mirrors agree with the suite registry exact set.
- Fingerprints cover every maintained verifier, focused test, manifest, and corpus owner.
- Candidate timing/RSS stays within fresh-main-derived, base-configured limits using the same cases and method.
- Final evidence records exact SHAs for local checks, remote CI, and review.

## Risks and rejected alternatives

- **Actions/native trust:** rejected because shared integration `15368`, candidate workflows, `pull_request_target`, and review ceremony cannot provide a distinct immutable check publisher.
- **Unapproved external control plane:** rejected; the private App and operating budget require separate authorization before precursor work.
- **Incremental extraction:** rejected by the atomic ruling and NO DUAL PATHS; it creates ownership ambiguity and cleanup debt.
- **Old/new dual execution:** rejected because it leaves two authorities; the immutable corpus is comparison authority.
- **Head-selected cases:** rejected because delayed weakening activates after merge; default-frozen deletion and two-stage retirement close it.
- **Raw YAML fallback:** rejected because text scans cannot preserve GitHub types, duplicates, or source semantics.
- **Network/unpinned or ambient bootstrap:** rejected because availability and parser behavior would vary before CI lint.
- **Vendored PyYAML or ruamel wheelhouse:** rejected as an update-burden fork or unnecessary second dependency model.
- **Python Mergify parsing:** rejected because Ruby/Psych remains the sole owner.
- **Bulk re-exports:** rejected because they preserve legacy coupling under a new name.
- **Permanent line fence:** rejected because the ceiling is cutover evidence, not runtime policy.

## Active-ledger context hygiene

The active #1016 ledger records only fresh-`main` facts and exact object IDs.
After the single precursor merges, refresh policy-base SHA, caller closure, rule manifest, corpus digest, timing, memory, and blockers; mark superseded branches/plans reference-only.
Separate accepted Program-A semantics, the permanent precursor, frozen #1016 scope, deferred packets, and later programs.
A tracker entry does not waive missing accepted scope, and deferred work is not missing #1016 scope unless it belongs to the central architecture above.
Every completion claim maps to a named test, static check, source-fence result, exact-head CI result, direct inspection, or explicit blocker.

## Later program map

| Later separately issue-owned program | Boundary retained by #1016 |
| --- | --- |
| CI provenance | Narrow adapter only; no policy/topology refactor |
| Merge readiness | Structured findings only; no readiness-policy change |
| Coverage enforcement | Existing owner remains; no registry absorption |
| Merge-queue execution | Consumes the result; mechanics remain unchanged |
| Rust verification | Existing owner and policy mirrors remain; only cutover pins change |
| Storage | Existing owners remain; no audit or retention redesign |
| AI review | Existing source/model governance remains untouched |
| Run fences | Existing owner remains; no consolidation |
| Operator tools | Existing commands and authority boundaries remain unchanged |

Each later program starts from merged `main`, declares its own issue slice, and supplies its own evidence.
None may be included in the #1016 cutover.

## Remaining blockers

Publication and implementation are blocked until the issue body explicitly reconciles its incremental wording with the approved atomic ruling.
#1016 precursor work is also blocked until separately authorized external-App/check/ruleset control and operating budget exist, or the Enterprise/org-transfer fallback is approved and complete.
Freeze is blocked until the one-shot envelope is revoked and protected state passes the closed canary, exact tuple, hermetic bootstrap, materializer, and fail-closed protocol evidence.
Cutover is blocked until fresh-main regeneration records the complete caller closure, rule manifest, corpus, timing, and memory baselines.
These are evidence gates, not permission to expand scope.

## Spec self-review

- Unresolved-marker scan: complete; no deferred implementation markers remain.
- Bootstrap/protocol scan: the one-shot envelope validates only precursor installation; no legacy translator or reusable bypass exists, and #1016 is the first real protocol candidate.
- Bootstrap-authority scan: App-owned signed conformance treats head assets as untrusted, exact reviewed bytes alone promote, and the monotonic tombstone rejects restoration/reissue.
- Derived-head scan: PR feedback and exact Mergify/merge-group authority are separate, event-specific, parent/tree/constituent-bound results.
- Policy-base scan: `policy_base_sha` selects policy; merge base is diff-only; terminal success revalidates the full authority record.
- Case scan: materialization separates head/mutated bytes, hides case identity/expectation, and binds nonce/digest.
- Freeze scan: every covered change is barred until atomic merge; post-cutover evolution is isolated/two-stage.
- Parser scan: PyYAML 6.0.2, CPython 3.12, safe-loader semantics, shared recipe, and failure classes agree.
- Syntax/scope scan: syntax classes and semantic owners are separate; the App contains no parser, corpus interpretation, or policy logic.
- Caller/budget scan: generated exact dispositions and full-path cross-language accounting prevent closure or ceiling gaming.
- Contradiction/ambiguity scan: issue conflict, authority, retirement, and deletions are explicit blockers/contracts.
- Scope scan: only central verifier architecture is owned; later programs are boundaries.
- Dual-path scan: the permanent seam precedes one atomic replacement and no legacy path survives.
- Evidence scan: every changed invariant and material risk maps to the evidence matrix.
