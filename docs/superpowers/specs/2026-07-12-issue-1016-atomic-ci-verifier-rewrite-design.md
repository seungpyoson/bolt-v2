# Issue #1016 Atomic CI Verifier Rewrite Design

## Decision and status

Issue #1016 will replace the central CI workflow-hygiene verifier atomically, but only after the complete replacement has been staged dormant on protected base. The protected `policy_base_sha` implementation is the sole semantic engine for the authority run. The atomic head may activate exactly the staged bytes, migrate callers and lifecycle-owned memberships, and delete the legacy path; it may not introduce or execute semantic implementation changes.

The user's goal is straightforward: CI must prove real safety from one owner, and an ordinary configuration change must not require Python or Python-test edits. Line count is secondary.

Only two ordering decisions are already approved: Program-A issue-owned deletions landed before the precursor, and actual legacy deletion occurs atomically with activation rather than in the precursor. The proposed temporary admission lock, temporary Mergify Merge Protections/Freeze ceremony, pre-precursor final ruleset state, promotion, irreversible bootstrap disablement, closed canary, and activation hinge remain pending separate control-plane approval, disposable live proof, owner approval, and external adversarial review, as do the dormant-implementation correction, two-context check design, and separately governed control plane. Publication and implementation require those approvals after the issue-body conflict is reconciled.

Two operational owner decisions are additionally explicit and unresolved: approval of a rehearsal-derived successful-ceremony target, retry count/time budget, and pre-precursor abort threshold; and separate acceptance that the post-precursor no-recovery posture can produce an unbounded terminal-tail ordinary-merge outage. The ordinary/unrelated-merge outage starts at admission-lock merge. No number or calendar bound is selected by this design.

This revision explicitly supersedes the earlier self-grading architecture, the claim that #1016 is the first ordinary head to speak a new protocol, and the proposal to introduce the replacement implementation in the same head that deletes the legacy implementation.

## Authority, baseline, and scope

This planning pass is based on protected main `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`. Current operational status is recorded in [`docs/ci/1016-program-ledger.md`](../../ci/1016-program-ledger.md); commit-qualified historical copies are evidence only.

Issue #1016 owns central-verifier architecture: classifiers, parsers, typed snapshots, semantic dispatch, retained adapters, stable protocol, materialization, corpus, focused evidence, one public facade, lifecycle membership, caller migration, and atomic legacy deletion. It does not authorize runtime-policy deletion, an external GitHub App, its budget, or later subsystem rewrites.

The independently supported baseline is 113 `scripts/*.py` files and 143,819 lines; the central retirement cluster is 26,667 lines. The 13,333-line attributable replacement ceiling remains a secondary, one-time governance limit. It is not empirical proof, a runtime fence, or permission to trade safety for LOC.

## Safety burden and rule disposition

Before freeze, #1016 requires a complete exact-SHA disposition inventory for every affected rule. Each row records stable control ID, exact path and symbol/span, executable risk, semantic owner, native-owner comparison, independent falsifying mutation or exact-identity proof, tier, frequency, cost, disposition, owner, issue, deadline, and evidence.

Only independently proven retained, re-homed, or native-owned rules enter the frozen #1016 corpus. Unresolved rows remain outside it and block the applicable cutover. Issue #1016 changes verifier architecture; it does not silently decide runtime-policy deletion.

`evidence_pending` permits only unchanged temporary retention. It requires an owner, issue, missing-proof statement, and review deadline, and it blocks cutover if the control would enter the frozen corpus. Expiry preserves the control and renews the blocker; it never defaults to deletion, de-automation, weakening, or re-homing. Missing proof for a proposed new required control rejects that proposal.

## Governing invariants

1. The candidate cannot choose the authority check, its publisher, policy base, semantic implementation, applicability, corpus, or success conversion.
2. Protected `policy_base_sha` supplies the sole semantic engine. Candidate bytes are governed inputs only.
3. The atomic head must exactly match a protected-base pending-activation manifest and leave every staged replacement byte unchanged.
4. Dormant conformance and focused tests are installation evidence, never a second enforcement path.
5. There is no legacy protocol adapter, old/new comparison, candidate-selected policy, dual semantic veto authority, ambiguous success conversion, compatibility layer, cleanup-later phase, or semantic AND window. The temporary admission lock and Mergify Merge Protections/Freeze are transition controls, never permanent semantic authority: legacy is the sole semantic veto for the admission-lock and precursor merges; no merge occurs between precursor and activation; and trusted authority is the sole semantic veto for activation and afterward.
6. Missing, malformed, timed-out, crashed, cancelled, replayed, incomplete, unknown, neutral, or skipped authority outcomes do not authorize merge.
7. Every retained semantic rule has one owner, one typed contract, and independent clean/falsifying evidence or exact native-owner identity proof.
8. Each governed syntax has one restricted parser; retained adapters consume typed inputs and cannot reopen or reparse governed files.
9. An ordinary configured value has one lifecycle authority and requires no Python, test, fixture, or manual digest edit.
10. The final cutover leaves one active enforcement implementation and deletes the full legacy implementation, tests, helpers, aliases, and pins in the same head.

## Two-stage trust and cutover

### Stage 1: permanent precursor on protected base

Before the admission lock lands, operators open the draft precursor to reserve its exact PR number, bring its implementation to review-ready and green under the approved exact-head evidence, and reserve a draft activation PR whose content is prepared to the exact allowed scope as far as possible. Control-plane and live canaries must be complete, and the pre-precursor abort/restore procedure must be reviewed. Operators then land one separately reviewed temporary `.mergify.yml` admission-lock PR under current legacy authority. Its protected configuration has exactly one ceremony queue matching only that precursor PR number, `batch_size: 1`, `max_parallel_checks: 1`, and `branch_protection_injection_mode: none`. It explicitly requires legacy `gate`, `backtester-gate`, `actionlint`, `host-health`, and mandatory native review. It exposes no hotfix, default, autoqueue, checkbox, automation, or other route for the activation or any unrelated PR. The lock merge makes the precursor's pre-lock green evidence stale: operators must refresh the precursor onto post-lock `main` and regenerate exact-head evidence before queueing, the rehearsal-derived outage budget must include that refresh, and only the fresh exact proof-head checks are merge-authoritative.

The ordinary/unrelated-merge outage begins when that admission-lock PR merges, not when Freeze is later enabled or when the precursor merges. During it, the precursor and activation ceremony PRs are the only permitted merges. On the successful path, the outage ends only after the activation has terminally proved protected `main` and the temporary controls have been removed. A disposable rehearsal derives a successful-ceremony target and elapsed-time/retry budget, plus a pre-precursor abort threshold; it does not establish a maximum overall outage. Before the admission-lock merge, the owner must approve those rehearsal-derived values and separately accept that terminal failure after precursor may leave the ordinary-merge outage unbounded because this design provides no recovery path. If the abort threshold is exceeded before precursor merge, operators execute the reviewed pre-precursor abort/restore procedure. After precursor merge there is no abort or repository recovery path.

After the separately approved and live-proven transition ceremony below, one precursor stages the complete replacement dormant: controller, classifiers, restricted parsers, typed snapshots, semantic registry, retained adapters, protocol validator, runner, materializer, corpus, manifest, bootstrap, focused tests, and an exact pending-activation manifest. The same precursor atomically replaces the temporary admission lock with final hotfix and default rules in which only legacy `gate` is replaced by App-qualified `trusted-ci-verifier`; `actionlint`, `backtester-gate`, and `host-health` remain explicit and subject to independently proven non-covered ownership.

The temporary protected-base admission lock governs the precursor merge and requires all four legacy checks plus mandatory native code-owner review. Because Mergify is placed in ruleset `exempt` mode before the precursor, it does not inject the final ruleset checks; `branch_protection_injection_mode: none` independently forbids such injection. The legacy verifier therefore remains the only repository semantic authority through the precursor merge. Dormant conformance proves installability and expected failure behavior; it neither publishes merge authority nor compares old and new verdicts. The bootstrap remains promotion-only and emits no check. There is no ordinary-PR authority-context exception.

Before queueing the precursor, all enforcement mutations are complete. Operators enable and prove Mergify Merge Protections and an indefinite `main` Freeze as temporary controls, initially excluding only the exact precursor PR number. While all other paths are frozen, they atomically place ruleset 14763242 in its final state: replace only the Actions-published `gate` requirement with external-App `trusted-ci-verifier`, retain `backtester-gate`, `actionlint`, and `host-health`, and change Mergify integration 10562's bypass mode from `always` to `exempt`. Per Mergify's documented ruleset behavior, `exempt` skips rule injection. Native/direct merges remain blocked because trusted authority is absent on ordinary heads, while the exact-number admission lock and Freeze constrain Mergify.

Operators then add the already-reserved exact activation PR number as the second Freeze exclusion. This exclusion is procedural only and cannot authorize merge; it is inert before precursor merge because the admission lock rejects the activation number and every non-ceremony route. Terminal proof re-queries protected main, `.mergify.yml`, Freeze and exclusions, ruleset, bypass mode, queue configuration, publisher identities, and queue/batch state; every other entry is dequeued and no batch may be running. Any protected-state movement restarts the pre-precursor ceremony. The precursor is then queued alone under the temporary admission lock. After it merges, the precursor exclusion is closed, the activation becomes routable under the final config, but trusted authority remains withheld and Freeze blocks every other PR. No legacy, alternate, or fallback semantic authority can authorize a post-precursor merge.

After merge, a signed bootstrap envelope authorizes only exact-byte verification and promotion; it never emits or satisfies a check. The external authority verifies the exact reviewed bytes at protected `policy_base_sha` and promotes them, writes the external monotonic tombstone, and irreversibly disables all later bootstrap issuance and acceptance. No repository, Mergify, ruleset, Freeze, or other enforcement mutation is permitted between precursor and activation; only promotion, tombstoning, and the closed canary occur. The control plane terminally re-queries final `main`, protected `.mergify.yml`, ruleset, protected ref, Freeze, bypass mode, queue/batch state, and publisher/App identities. The canary then runs under Freeze against the promoted protected-base engine, final authority tuple, and post-disable state. Its independently precommitted cases include at least one known-clean input and one known-falsifying input. It passes internally only when the clean case computes allow, the falsifying case computes deny for the expected stable rule ID, all complete protocol and tuple checks pass, no bootstrap issuance or acceptance succeeds, and no preexisting proof-head result survives any protected-state movement. The canary cannot create, update, publish, or satisfy `trusted-ci-verifier` or any other merge-visible status or check. Its nonce, authority record, artifacts, and computed results are canary-purpose evidence only and cannot be converted, promoted, replayed, or reused as activation authority. `trusted-ci-verifier` remains absent until the reserved activation is queued alone and the activation proof head passes complete terminal tuple validation.

On canary success, exact freeze evidence is recorded as non-authorizing control-plane audit evidence and the already-reserved activation queues alone. That record is not a status or check and cannot satisfy a merge condition. The activation is literally the first subsequent covered Git/enforcement-surface change after the precursor. The App emits `trusted-ci-verifier` only for its exact Mergify proof head after validating ancestry, protected base, head, tree, manifest, staged-byte identity, final ruleset/configuration, exact constituent set, and queue/freeze state. It merges only when temporary procedural controls and trusted semantic authority both pass.

After activation, Freeze remains until protected `main` exactly equals the expected activation result and terminal proof succeeds. Operators then delete the temporary Freeze and disable or remove temporary Merge Protections reporting and any temporary required binding. The final App-qualified trusted ruleset requirement and Mergify `exempt` bypass remain unless a later separately authorized design changes them.

Before precursor merge, abort restores the pre-ceremony state through the separately reviewed operator procedure. After precursor merge, a terminal canary or activation failure has no recovery PR, recovery exclusion, or repository recovery path: Freeze remains active, the attempt is terminally abandoned, bootstrap stays tombstoned, and the repository may remain unable to merge. Proceeding therefore requires explicit owner risk acceptance and a separate external operational decision. Any recovery is a new, separately authorized program. Any movement in the exact base, configuration, or covered surface invalidates all prior proof and restarts the pre-precursor ceremony where still possible.

Post-precursor retries distinguish observation or execution noise from a failed proof on the merits. A retry is allowed only while protected `main` and its base, protected `.mergify.yml`, ruleset and bypass state, Freeze and exclusions, staged bytes and manifest, activation head, allowed constituent set, and bootstrap tombstone remain byte-for-byte or identity-for-identity unchanged. Stale reads, API or network timeouts, a cancelled runner, a check rerun, a blocked extra queue entry, or regeneration of the proof head for the same exact activation and base may be dequeued, re-queried, or retried with a fresh nonce. A non-publishing canary infrastructure failure may likewise rerun. Failed, intermediate, infrastructure-noise, canary, reused, converted, or partial retry artifacts or results cannot publish merge-visible authority. A fresh activation retry may use a fresh nonce with the same immutable activation/base/configuration tuple and may emit a new `trusted-ci-verifier` result only after independently completing the entire terminal validation for its exact regenerated proof head. It cannot reuse or convert any prior result. Retries remain bounded by the rehearsal-derived count and elapsed-time budget fixed and owner-approved before the admission-lock merge; exhaustion is terminal.

A canary allow/deny or stable-rule-ID failure on the merits, a protocol, tuple, or monotonicity failure on the merits, mutation of any immutable protected/configuration/staged/activation identity, an unauthorized constituent, or an activation exact-manifest or semantic failure is terminal. Terminal failure retains Freeze and the no-recovery posture above. Queue noise is therefore retryable only when it is proven not to have changed any immutable identity; it is never silently reclassified as semantic success.

### Stage 2: atomic #1016 activation

The atomic head may only:

- match the base-owned pending-activation manifest exactly;
- keep every staged semantic, runner, protocol, parser, corpus, bootstrap, and test byte unchanged;
- switch the staged implementation from dormant to active;
- migrate direct and dynamic callers plus lifecycle-owned suite/lane/fingerprint membership;
- update provenance-bound generated artifacts deterministically; and
- delete all legacy implementation, tests, helpers, registrations, aliases, and obsolete pins.

The activation manifest also removes legacy covered-semantic execution and registration from the required gate. Any semantic replacement edit or extra activation-surface change makes authority absent or failing. The base-owned staged implementation evaluates the atomic head, including its activation and deletion set. After cutover, no candidate-controlled required GitHub Actions check may retain a covered-semantic veto.

### Future verifier versions

While Vn is sole authority, a separate PR stages Vn+1 dormant with its exact pending-activation manifest. That staging PR merges through an exact Mergify proof head judged by protected-base Vn. Only while a separately approved procedural freeze is already active, and after protected-base promotion, applicable final-authority establishment and validation, a successful closed canary, and exact base/config/surface-bound freeze evidence, may an isolated activation PR judged by base-owned Vn+1 activate Vn+1 and atomically delete Vn. No same-event comparison or candidate self-grading is permitted.

## Check contexts and Mergify authority

The repository uses Mergify rather than native `merge_group` as its normal merge authority. Live ruleset 14763242 gives Mergify App 10562 an always-bypass, and a bare check name does not bind its publisher.

Two external-App contexts have distinct purposes:

- `trusted-ci-verifier-feedback`: non-required ordinary-PR feedback only. It can never satisfy a ruleset or Mergify merge condition.
- `trusted-ci-verifier`: merge authority only. During the initial ceremony it is absent from ordinary PR heads, promotion, the closed canary, and every proof head except the exact reserved activation proof head after complete terminal tuple validation. After successful activation, terminal protected-main proof, and temporary-control removal, it becomes the normal steady-state merge authority for each exact Mergify proof head evaluated by the currently active protected-base verifier and passing complete terminal tuple validation; native `merge_group` is eligible only if separately enabled and governed. It remains absent on ordinary PR heads. Wrong phase, head, publisher, constituent, or identity fails.

Both contexts are published by the external App. There is no GitHub Actions feedback or authority fork, and readiness is evidence-driven rather than tied to an arbitrary calendar deadline.

Protected Mergify configuration requires the App-qualified authority name, conceptually `check-success = @<external-app-slug>/trusted-ci-verifier`. The protected GitHub ruleset separately binds `trusted-ci-verifier` to the exact App integration ID. The actual installed App slug is an installation output and must not be a hardcoded placeholder in production configuration.

The staged base-owned semantic registry includes stable rule `merge.authority.app-qualified-condition`. It validates only the exact protected `.mergify.yml` condition structure and both hotfix/default queue-rule mappings. Live App publisher/integration identity and the ruleset binding are validated exclusively by the external authority-tuple/control-plane validator, never by repository semantic code. `merge_queue_preflight` remains advisory only and cannot authorize merge.

Authority is invalidated by mismatch or movement in base, head, parent, tree, constituent set, installation identity, launcher or artifact identity, ruleset epoch or digest, Mergify configuration, or activation manifest. Reuse, cancellation, replay, timeout, crash, malformed protocol, extra activation changes, stale API state, or terminal revalidation failure also invalidate it. Neutral and skipped conclusions are forbidden because GitHub accepts them as successful required conclusions.

### Exact authority tuple and bootstrap lifecycle

Every result is bound to one signed authority record containing repository ID; protected base repository and ref; external App integration and installation IDs; context name and purpose; ruleset ID, version, and digest; Mergify configuration digest and epoch; launcher SHA and artifact digest; exact `policy_base_sha`; proof `head_sha`; event kind and delivery ID; exact proof-head and branch identity; proof parent and tree identity; ordered constituent heads where applicable; activation-manifest digest; and staged-version identity. A bound tuple digest may compact the protocol record only when required identities remain explicit and protected validation reconstructs and verifies the complete tuple.

`policy_base_sha` is the exact protected base of the ordinary PR or proof head. Merge-base computation selects diff bytes only and is never policy or semantic authority. Immediately before terminal success, the App re-queries every identity, ancestry, object-existence, parent/tree, constituent, protected-ref, configuration, ruleset, launcher, installation, manifest, staged-version, and digest field. Movement, missing objects, changed ordering, staleness, ambiguity, or mismatch fails closed; earlier successful queries are not terminal proof.

For the precursor only, the App may mint one signed, expiring, event-specific bootstrap envelope bound to the complete tuple, protected `main`, the exact reviewed allowlist, and every reviewed content digest. The envelope authorizes only post-merge exact-byte verification and promotion; it cannot publish, emit, satisfy, or waive a check. It has no generic renewal, wildcard, derived-head reuse, or signer override. After merge, the App verifies the exact reviewed bytes at protected `policy_base_sha`, promotes those bytes, and records an external monotonic tombstone keyed to the precursor SHA once that SHA is an ancestor of protected `main`. Every later bootstrap issuance and acceptance fails regardless of signer. Replay, reissue, datastore rollback, and restoration of an old App artifact must fail before authority establishment, canary, or freeze.

### Temporary transition controls and proof

There is no permanent `Mergify Merge Protections` required check. Before the precursor, separately approved operators use Merge Protections and Freeze only as temporary ceremony controls. Ruleset 14763242 reaches its final state before the precursor: only the Actions-bound `gate` is replaced by external-App `trusted-ci-verifier`, the other three checks remain, and Mergify integration 10562 changes from `always` bypass to `exempt`. The [GitHub Cloud repository-rules REST API](https://docs.github.com/en/rest/repos/rules) lists `always`, `pull_request`, and `exempt`, and defines `exempt` as rules not running and no bypass audit entry being created. That loss of GitHub-side bypass audit entries is an explicit trade: the compensating evidence is the control-plane's retained, digest-bound audit of every App emission plus the ceremony's signed terminal re-query receipt, sufficient for later incident reconstruction. The protected admission lock's `branch_protection_injection_mode: none` and Mergify's documented `exempt` behavior are proposed to prevent final ruleset injection into the precursor queue, subject to the disposable live proof below.

Disposable non-production proof must cover: this repository and API version accepting GitHub Cloud ruleset bypass mode `exempt`; Mergify injection behavior under that mode; exact-number admission; `branch_protection_injection_mode: none`; absence of hidden hotfix/default/autoqueue/checkbox/automation routes; configuration self-change resetting prior proof; invalidation of a preexisting proof head; single and mixed batches; merge-time Freeze re-evaluation; exact exclusions; dequeue and no-running-batch state; wrong-publisher same-name rejection; native/direct blocking; Freeze behavior when Mergify is exempt; exact identity; latency; bounded retry classification and exhaustion; successful-ceremony timing and a pre-precursor abort threshold; and API, quorum, and audit behavior. Until all results are reviewed, the owner approves the rehearsal-derived target, retry budget, and abort threshold, separately accepts the unbounded post-precursor terminal-tail risk, and the separate control-plane design is approved, implementation remains blocked. The official bases are [GitHub Cloud repository-rules REST API](https://docs.github.com/en/rest/repos/rules), which documents `always`, `pull_request`, and `exempt` bypass modes, [Mergify ruleset behavior](https://docs.mergify.com/merge-queue/github-rulesets/), [conditions](https://docs.mergify.com/configuration/conditions/), [batches](https://docs.mergify.com/merge-queue/batches/), [Merge Protections setup](https://docs.mergify.com/merge-protections/setup/), and [Freeze](https://docs.mergify.com/merge-protections/freeze/). Those documents do not replace the required disposable live proof, and this plan does not claim that any live rule has already been mutated.

The transition is exact replacement, never a semantic AND window. The separately landed exact-number admission lock is judged by legacy authority. The precursor atomically installs dormant bytes and replaces that lock with final Mergify mappings, but the lock and explicit legacy checks judge the precursor itself. Freeze prevents every other merge. The ruleset is already final; no post-precursor configuration or ruleset mutation occurs. Promotion, irreversible tombstoning, and the closed canary happen without a Git mutation. During this initial ceremony, trusted authority is emitted only for the exact activation proof head; after successful cleanup it follows the steady-state proof-head rule above. At activation, legacy covered-semantic execution and registration are removed from `gate`; `gate` may survive only for independently proven non-covered work and may not retain a covered-semantic veto. Ruleset, Mergify, Freeze, queue, base, or staged-byte movement invalidates the proof.

## Separately approved control-plane prerequisite

This design does not authorize the external App, its installation, ruleset mutations, or operating budget. A separate approved control-plane design must define:

- authorization principals, quorum, and digest-bound approval;
- source-to-artifact attestation for launcher and installation verifier;
- append-only, rollback-resistant bootstrap tombstone and proof-reuse state;
- key custody, rotation, revocation, and recovery;
- audit retention and incident reconstruction; and
- ownership of ruleset and Mergify installation changes; and
- authorization, quorum, audit, exact identity, and recovery rules for Merge Protections and Freeze operations.

The App may authenticate, resolve and revalidate authority records, sandbox the base-owned runner, validate its envelope, and publish the appropriate context. It may not contain hidden repository policy, parse governed documents, interpret the semantic corpus, or choose candidate semantics.

### Trusted computing base

The enumerated TCB comprises GitHub protected refs, ruleset evaluation, object and identity APIs, check-run publisher binding, and native-review enforcement; the Mergify merge/bypass executor, condition evaluator, queue/batch engine, Merge Protections/Freeze service, and protected `.mergify.yml`; and the external App, launcher, control plane, artifact store, keys, rollback-resistant state, and monotonic tombstone. Native code-owner review is an independent human authorization anchor for the admission lock, precursor, and activation, but it is not the semantic oracle. Residual risk includes compromise, equivocation, stale or misreported identity/configuration, bypass misuse, queue race, key loss, and rollback failure across those systems. Detailed prevention, detection, separately governed incident recovery, quorum, and audit controls belong to the separately approved control-plane design.

## Components and dependency direction

The staged replacement consists of independently reviewable units:

- **Runner/materializer:** combines exact candidate documents with base-owned clean and falsifying cases under bounded resources; records provenance without exposing an answer key to candidate code.
- **Classifiers and parsers:** classify GitHub workflow/action YAML, TOML, Just, shell, and Mergify/Psych inputs, load each through one restricted owner, and produce immutable typed snapshots.
- **Semantic registry:** maps each stable rule ID to exactly one typed owner or retained native adapter; unknown, duplicate, or unowned IDs fail.
- **Protocol validator/reporter:** validates canonical structured results, completeness, identity, order, and termination; rendering never reinterprets policy.
- **Bootstrap:** installs exact locked parser/runtime artifacts hermetically and fails closed on unsupported, missing, corrupt, ambient, or mismatched inputs.
- **Activation manifest:** binds every staged byte, public entrypoint, caller disposition, lifecycle membership, generated output, legacy deletion, and allowed activation edit.

`external App -> protected-base runner/materializer -> staged controller -> classifier -> restricted parser -> typed snapshot -> semantic owner -> protocol validator -> reporter`

The engine does not rely on secret case IDs, nonces, or a finite mutation set to prevent self-grading. Its trust comes from executing protected-base semantic code against candidate inputs.

### Materializer, parser isolation, and parse-once proof

The staged protected-base engine receives only immutable materialized bundles, never repository paths, a checkout mount, Git metadata, or ambient repository access. Actual-head bytes and mutated-case bytes retain distinct provenance through classification, parsing, findings, and reports. An unknown file inside a governed surface is `classification_failure`, never ignored.

The external sandbox mount proves byte scope. Counting-loader evidence separately records exactly one byte load and one parser or native-owner invocation per selected document. Static dependency and import fences reject raw file I/O or parser imports outside snapshot owners. Retained owners accept only stdin or immutable typed input and cannot reopen or reparse governed sources.

Snapshots expose only normalized jobs, events, permissions, steps, commands, dependencies, configuration facts, absence/type states, and source spans. Private variants and metamorphic mutations are focused semantic-test quality, not a trust root or proof of honesty.

### Stable protocol and fail-closed boundaries

The engine emits exactly one canonical UTF-8 JSON document and no other protocol-channel output. Its envelope contains protocol version; the complete authority tuple, or its bound digest plus required identities; an opaque invocation nonce; immutable bundle digest; terminal classification; and complete ordered findings.

Each finding contains a stable `rule_id`, opaque document token, and either a source span or typed detail. Protected validation maps tokens to normalized repository-relative paths. Nonce and digest bind one invocation and reject replay; they explicitly are not an honesty or self-grading trust root.

Protected validation rejects wrong, generic, duplicate, unknown, or unowned IDs; missing or wrong tuple fields, nonce, digest, or SHA; replayed, duplicate, or unselected invocations; malformed JSON, extra output, unknown fields, wrong types, or unsupported versions; swallowed parser errors; incomplete or unexpected findings; unstable ordering or lossy paths; and missing output, nonzero exit, crash, timeout, signal, or resource exhaustion. Paths reject absolute forms, traversal, and symlink escape.

Boundary classifications are `launch_failure`, `revision_failure`, `bootstrap_failure`, `classification_failure`, `parse_failure`, `protocol_failure`, `engine_failure`, and `semantic_failure`. Unknown classifications become `protocol_failure`. Runner failures cannot masquerade as semantic findings. Diagnostics are bounded and never expose credentials, environment contents, arbitrary binary output, or unbounded source.

### Restricted parsers and exact hermetic bootstrap

GitHub workflow and action YAML use PyYAML 6.0.2 through one GitHub-specific safe loader. A protected-policy-base complete hash lock provisions a content-addressed isolated CPython 3.12 environment. The platform matrix is CPython 3.12 local macOS arm64 plus CI manylinux x86-64 and aarch64 with exact wheel tags. Unsupported host or missing or extra wheel hash is `bootstrap_failure`.

The sole public recipe is `just provision-ci-verifier-python`. It scrubs `PYTHONPATH`, `PYTHONHOME`, and pip configuration and performs an atomic `pip --require-hashes --only-binary=:all: --no-deps` install with no ambient fallback. `ci-lint-workflow`, full and static source-fence, and all direct verifier entrypoints traverse this recipe. Exact-set closure rejects alternate locks, raw provisioning, ambient imports, and direct unprovisioned invocation.

A fresh network bootstrap and validated warm-offline cache must pass. Offline cache miss, network failure, hash mismatch, corrupt artifact, incomplete lock, or non-atomic install is `bootstrap_failure`. The content address and atomic ready manifest bind interpreter, lock, artifacts, recipe, and platform. Cache reuse verifies distribution/version, module origin, isolation, installed files, and digests. Poisoning, system-site leakage, wrong version, interrupted construction, missing or extra files, or digest mismatch fails.

The safe loader preserves `on` as a string; accepts valid trigger scalar, sequence, and mapping forms into one typed representation with source marks; and rejects duplicate keys, anchors, aliases, merges, tags, multiple documents, and wrong node types. There is no raw-text or regex fallback. TOML has one standard type-checking adapter, shell uses `shell_dataflow_analysis`, Just uses one native dump, and Mergify remains Ruby/Psych-owned. Vendored PyYAML and a second `ruamel.yaml` dependency model are rejected.

## Corpus evolution and delayed weakening defense

During the precursor-to-activation freeze, no covered corpus or manifest change is permitted, including additions. After cutover, strengthening additions land isolated from behavior changes and become selectable only from protected-base state.

Deletion, narrowing, applicability or expectation change, and rename require two governed stages. First, an owner-approved retirement record lands while the old case remains active; it names the stable rule, replacement, semantic-superset argument, and falsifying evidence. Later, an isolated protected-base activation applies the change only after exact-head proof and with no unrelated change. Renames use a base-owned one-to-one, time-bounded identity migration. Missing cases, unexpected applicability, and incomplete findings fail.

## Atomic facade and exact caller closure

`scripts/verify_ci_workflow_hygiene.py` remains the thin stable facade. It preserves no-argument and `self-authorizing-governance` modes unless every caller changes atomically, performs validation plus one delegation only, and exports no rules or compatibility symbols.

The activation manifest fixes this exact baseline closure; a generator from exact `policy_base_sha` rejects every unlisted direct, import, dynamic-load, symbol, command, fingerprint, or membership edge:

| Current edge | Atomic disposition and proof |
| --- | --- |
| Helper loads in `test_nextest_fingerprint.py`, `test_clean_merged_artifacts.py`, `test_workflow_expression_analysis.py`, `test_governance_diff_analysis.py`, `test_cargo_shim.py`, and `test_sandbox_safe_push.py` | Move each to its focused fixture owner; per-path dynamic-load closure proves deletion. |
| Helper loads in `test_rust_verification_decoupling.py`, `test_verify_bolt_v3_boundary_evidence.py`, `test_cargo_command_analysis.py`, `test_merge_queue_preflight.py`, `test_shell_dataflow_analysis.py`, `test_ci_input_sets.py`, and giant `test_verify_ci_workflow_hygiene.py` | Move each to its focused owner; re-home the generic Rust/NO DUAL PATHS invariant first; delete helper and giant test with zero residual loads. |
| `test_command_understanding` compatibility exports | Import the canonical command-analysis owner and delete compatibility exports; symbol/import exact-set proof applies. |
| `test_ci_storage_tripwire` direct call | Use the canonical storage-tripwire typed adapter; direct-call scan and clean/falsifying cases apply. |
| `merge_queue_preflight.py` authority evidence | Validate separate feedback/authority contexts and the App-qualified exact proof; wrong App, context purpose, launcher, ruleset, event, policy base, proof head, parent/tree, or constituents fail. |
| `verifier_io.py` | Own the stable facade/protocol command; CLI exact-set and dynamic invocation proof apply. |
| `ci/doc-decoupling-residuals.toml` | Remove resolved legacy residuals and retain only independently valid entries; exact-set and stale-symbol scan apply. |
| `run_ci_lint_suites.py` | Consume the one lifecycle registry; missing, duplicate, and extra-suite mutations fail. |
| `ci/rust-verification.toml` and `crates/backtesting-vertical-slice/ci/rust-verification.toml` | Derived, non-authoritative #1016 membership consumers; zero-diff regeneration proves they are not manual mirrors. |
| `.github/workflows/ci.yml` fingerprint and trusted-base edges | Cover runner, lock, manifests, corpus, code, tests, and protected-base authority; fingerprint/input-closure mutations fail. |

All 13 helper consumers receive focused owners. The generic NO DUAL PATHS rule is re-homed and proven before its historical test and pins disappear. The giant test and helper are deleted, with zero aliases, compatibility exports, comparisons, or legacy entrypoints left in the final head.

## Later-program boundaries

| Separately issue-owned program | Boundary retained by #1016 |
| --- | --- |
| CI provenance | Narrow typed adapter only; no provenance policy or topology rewrite. |
| Merge readiness | Structured findings only; no readiness-policy change. |
| Coverage | Existing owner remains; no registry absorption. |
| Merge mechanics | Mergify/proof evidence only; queue and merge mechanics remain outside #1016. |
| Rust verification | Existing domain owner remains; only derived #1016 membership consumers change. |
| Storage | Existing owners remain; no audit or retention redesign. |
| AI review | Existing source/model governance remains untouched. |
| Run fences | Existing owner remains; no consolidation. |
| Operator tools | Existing commands and authority boundaries remain unchanged. |

## One lifecycle membership authority

#1016 introduces one lifecycle-owned declarative registry for only these #1016-owned memberships:

- central verifier suite registration;
- the focused cheap-lane membership consumed by the two existing TOML locations; and
- central-verifier workflow fingerprint inputs.

Consumers read the registry directly where feasible. An unavoidable standalone artifact is deterministic, explicitly non-authoritative, and provenance-bound to generator, canonical input, command, and digest. A zero-diff regeneration check enforces derivation. Tests validate registry schema and derivation rules; they do not restate exact membership sets.

This design does not claim a global registry for unrelated Rust-verification topology. Any such consolidation requires a separate issue-owned topology design and proof.

Maintenance evidence must demonstrate one member change as one human authority edit with no Python, test, or manual-digest edits. Raw changed files/lines and deterministic derived-byte churn are reported separately.

## Change classes and accounting

The hard semantic requirements are:

- **A — ordinary value:** one lifecycle authority, no manual mirrors, and no Python/test/manual-digest edit.
- **B — existing generic semantics:** no new branch, regex, copied fixture, or test method.
- **C — new semantic invariant:** one semantic owner plus independent clean and falsifying proof.
- **D — topology:** one topology authority; no mirrored literal, copied workflow, or manual digest.
- **E — advisory/operator/reporting:** cannot authorize a required merge or deploy.

Classification precedence is E when non-authoritative; otherwise D > C > B > A. Pure moves, refactors, documentation, and provenance renewal have no A–E amplification ratio.

One receipt represents one semantic intent, lifecycle owner, and rollback boundary. Every changed hunk belongs to one receipt; shared infrastructure is a separate receipt. Representation amplification is the number of manually maintained fact-to-representation edges touched divided by canonical semantic facts changed. Candidate-added helper fields do not increase the denominator. Raw files/lines and derived outputs are reported separately.

Every symbol/span record has two axes:

- functional role: `authority`, `implementation`, `evidence`, `provenance`, or `docs`;
- representation origin: `unique`, `derived`, or `duplicated`.

Evidence and digests may therefore be duplicated or derived. Independent evidence requires a separate oracle owner and a killed mutation.

All ordinary numerical line, file, ratio, percentage, and rolling-median values are provisional review signals until a reproducible calibration set exists. They are not gates, acceptance criteria, or automatic exception machinery. The explicit exception is the 13,333-line #1016 ceiling: it is a one-time owner-selected governance ceiling secondary to semantic minimality, not empirical evidence or a runtime CI fence. The historical method and supported subtotals live in [`docs/ci/1016-receipts/9f3b13f/change-amplification-baseline.md`](../../ci/1016-receipts/9f3b13f/change-amplification-baseline.md).

Deterministic `just ci-verifier-budget-report` and a protected-base path/symbol inventory attribute full paths across all languages, including launcher seams, runner/protocol, facade, parsers, registry, adapters, bootstrap, measurement, focused tests, and generated executable code. Non-executable corpus and manifest exclusions are validated and reported separately. Corpus and manifest data may carry governed inputs and expected evidence, but never executable rule selection, applicability, or verdict logic; only typed materializer/runner evidence paths may consume them. Import/dataflow fences and hidden-policy mutations enforce that boundary. Hidden policy in data, generated output, retained owners, or another language is counted and rejected.

Non-Python repository code and external-App code are visible subtotals. App operations, maintenance, and hosting cost remain separate and visible. Semantic policy in the App is forbidden; permitted non-semantic conformance and control-plane code is still cost-accounted. This deterministic report is cutover evidence, never a permanent ordinary-PR line fence.

## Requirement-to-evidence matrix

| Requirement or risk | Required evidence |
| --- | --- |
| Immutable launch | Candidate workflow deletion cannot suppress either context; publisher/App binding and launcher artifact are exact. |
| Installation authority | Signed precursor envelope is allowlist/digest-bound; promoted bytes exactly equal reviewed bytes. |
| Temporary lock prerequisite | Before the separately reviewed admission-lock PR lands under current legacy authority: precursor is draft-numbered, review-ready, and green; activation is draft-numbered and prepared to exact allowed scope as far as possible; control-plane/live canaries and the reviewed abort/restore procedure are complete; and the owner accepts the rehearsal-derived successful-ceremony target, retry budget, pre-precursor abort threshold, and separately the unbounded terminal-tail outage risk after precursor. The lock then proves exact precursor-number matching, one queue, batch size one, one parallel check, injection disabled, explicit four legacy checks, native review, and no alternate route. |
| Final pre-precursor state | Disposable live proof covers this repository/API accepting `exempt`, Mergify injection under it, exact-number admission, no hidden routes, self-change reset, proof-head invalidation, mixed batches, merge-time Freeze re-evaluation, exclusions, dequeue/no running batch, wrong publisher, native/direct blocking, Freeze under exempt, identity, bounded retry classification/exhaustion, successful-ceremony timing and a pre-precursor abort threshold, latency, and API/quorum/audit. Terminal re-query proves final ruleset, bypass, Freeze, configuration, protected base, and empty queue/batch state before precursor. |
| Authority establishment | Ruleset 14763242 is final before precursor; temporary admission lock and explicit legacy checks judge precursor; precursor atomically installs dormant bytes and final Mergify mappings; no enforcement mutation follows before activation. |
| Exact check map | Final Mergify hotfix/default rules replace only `gate` with App-qualified `trusted-ci-verifier` and explicitly retain `actionlint`, `backtester-gate`, and `host-health`; final ruleset 14763242 makes the same gate-only replacement and retains the same three checks. No permanent Merge Protections requirement exists; Mergify stays `exempt`. |
| Failed-canary terminal state | Canary is internal and non-publishing; it cannot create or satisfy a merge-visible context, and none of its records or artifacts can become activation authority. On failure, Freeze stays active, bootstrap stays tombstoned, no recovery PR or exclusion exists, and the repository may remain unable to merge. Recovery requires a new separately authorized program. |
| Retry/terminal boundary | Only enumerated infrastructure/observation noise may retry with a fresh nonce while every protected, configuration, staged, activation, constituent, and tombstone identity is unchanged. No failed, intermediate, infrastructure-noise, canary, reused, converted, or partial retry result can publish authority. A fresh activation retry may emit a new result only after independently completing terminal validation for its exact regenerated proof head under the same immutable tuple. Retries are bounded by pre-lock owner-approved rehearsal budgets; exhaustion or any merits/identity failure is terminal. |
| Proof-head merge authority | Feedback cannot authorize merge; wrong proof head, branch, parent/tree, constituent, context purpose, or App fails. |
| Steady-state authority | After successful activation, terminal main proof, and temporary-control removal, each exact Mergify proof head is judged by the active protected-base verifier; authority remains absent on ordinary PR heads, and Vn judges Vn+1 staging. |
| Bootstrap/tombstone closure | Replay, renewal, reissue, datastore rollback, and old-App restoration fail after monotonic tombstone. |
| Exact revisions and terminal re-query | Every tuple field, object, ancestry, digest, and protected ref is revalidated immediately before success. |
| Base semantic authority | Head edits to engine, corpus, manifest, protocol, or applicability cannot grade themselves. |
| Frozen applicability and corpus retirement | Freeze rejects additions and weakening; two-stage retirement/rename keeps the old case active first. |
| Stable protocol | Envelope fields, complete ordered findings, ID/nonce/digest/SHA/replay, malformed/extra output, and terminal failures are exercised. |
| Parser failures | Crash, unsupported syntax, wrong type, swallowed error, and partial output fail under the correct boundary class. |
| Parse once | Sandbox mount, counting-loader, and dependency/import fences prove byte scope and one owner invocation. |
| Restricted YAML | Valid trigger scalar/sequence/map pass; duplicate, anchor, alias, merge, tag, multi-doc, and wrong-type cases fail. |
| Hermetic bootstrap | Fresh-network and warm-offline pass; miss/network/hash/corruption/lock/platform/cache-poison cases fail. |
| Singular ownership | Registry exact-set maps every stable rule to one owner and typed contract. |
| Semantic continuity | Every retained or re-homed rule has independent clean and falsifying evidence or exact native-owner identity proof. Hidden/metamorphic cases are focused evidence only, not authority proof. |
| Generic dual-path rule | Mutations distinguish competing active owners/paths from valid independent components. |
| Facade | No-argument and `self-authorizing-governance` modes pass end-to-end caller tests. |
| Caller migration | Exact closure finds zero helper/giant/legacy imports, loads, symbols, aliases, pins, or comparisons. |
| Membership derivation | Registry, suite runner, two derived TOML consumers, and CI fingerprint edges regenerate with zero manual mirrors. |
| Resource budget | Identical protected-base cases, configuration, and method compare candidate timing/RSS against fresh-main-derived base-configured limits; runner minutes, external calls, storage, and cost are also reported. |
| Final proof | Relevant cheap gates and exact-head required CI/equivalent evidence exist at the reviewed SHA. |

Each row is bound to command, exact input SHA, output digest, and reviewer. Missing evidence blocks the relevant phase. Dormant conformance proves installation only; final authority comes from the protected-base engine on the exact proof head.

## Delivery and review sequence

1. Reconcile the issue body and record owner decisions; separately authorize the App/control plane and the proposed Merge Protections/Freeze mechanism.
2. Regenerate exact-main rule, caller, corpus, resource, cost, and attribution evidence.
3. Assign one implementer per declared file set; resolve internal findings and run cheap checks first. Open the draft precursor to reserve its exact PR number and build its complete dormant replacement/pending-activation manifest plus atomic replacement of the admission lock by final hotfix/default Mergify rules. Make it review-ready and green under the approved exact-head evidence. Reserve a draft activation PR and prepare it to its exact allowed scope as far as possible.
4. In disposable non-production state, prove the complete temporary Merge Protections/Freeze matrix and rehearse the successful ceremony. Complete control-plane/live canaries and review the pre-precursor abort/restore procedure. Fix the rehearsal-derived successful-ceremony target, retry count/time budget, and pre-precursor abort threshold; obtain explicit owner approval of those values and separate acceptance that terminal failure after precursor can create an unbounded ordinary-merge outage because there is no recovery path.
5. Land one separately reviewed temporary admission-lock PR under legacy authority with exactly one precursor-number queue, batch size one, one parallel check, injection disabled, explicit four legacy checks, native review, and no alternate route. The ordinary/unrelated-merge outage starts when this lock merges; only the precursor and activation ceremony PRs are permitted until successful cleanup. Refresh the precursor onto post-lock `main` and regenerate exact-head evidence before queueing because its pre-lock green evidence is now stale; include that refresh in the rehearsal-derived outage budget and treat only fresh exact proof-head checks as merge-authoritative.
6. Create an indefinite `main` Freeze initially excluding only the precursor, atomically establish the final ruleset and Mergify `exempt` bypass state, and add the already-reserved activation number as its inert second Freeze exclusion.
7. Terminally re-query main, config, Freeze/exclusions, final ruleset, bypass, identities, exact-number queue, and no-running-batch state. Restart on movement; dequeue all other entries; queue precursor alone under the admission lock, legacy checks, and native review.
8. After precursor merge, make no enforcement mutation. Promote exact bytes, tombstone bootstrap, irreversibly disable later issuance/acceptance, and run the internal, non-publishing closed canary against final state. It may compute allow/deny but cannot create or satisfy any merge-visible context, and its evidence cannot become activation authority. Retry only enumerated infrastructure/observation noise while every immutable identity is unchanged and within the pre-approved budget; failed, intermediate, canary, reused, converted, or partial results cannot publish authority. A merits/identity failure or budget exhaustion is terminal, keeps Freeze active, and has no recovery PR.
9. On canary success, queue the already-reserved activation alone as the literal first subsequent covered enforcement-surface change. A fresh retry uses a fresh nonce and the same immutable activation/base/configuration tuple; emit a new trusted result only for its exact regenerated Mergify proof head and only after independently completing the entire base/head/tree/manifest/staged-byte and terminal-state validation. Never reuse or convert a prior result.
10. After protected main equals the expected activation result and terminal proof succeeds, remove Freeze and disable/remove temporary Merge Protections reporting or binding; only then does the outage end. Retain final App-qualified trusted ruleset authority and Mergify `exempt` bypass unless separately redesigned. In steady state, trusted authority is emitted for each exact Mergify proof head evaluated by the active protected-base verifier and remains absent on ordinary PR heads.

This sequence claims neither readiness nor approval.

## Rejected alternatives

- Candidate-head semantic execution, hidden finite cases, nonce-based honesty, and candidate-selected corpus: candidate self-grading.
- Old/new comparison or a legacy protocol adapter: dual authority and ambiguous failure ownership.
- A single check context for feedback and authority: ordinary PR feedback could be mistaken for merge proof.
- Bare required-check names: they do not bind the publishing App.
- A Mergify-first PR that installs final authority, a post-precursor ruleset or exclusion mutation, a recovery manifest/PR, or an ad-hoc post-merge lock: each creates an unauthorized path or breaks the terminal pre-precursor state. The one permitted prior config PR is the exact-number temporary admission lock described above.
- Mergify pause or drain as the security lock: joining continues while paused.
- Manual suite/TOML/fingerprint mirrors or tests that restate exact sets: multiple authorities.
- Missing-evidence deletion, expiry deletion, or silent corpus retention: unproved weakening.
- Generated hidden policy, compatibility layers, permanent measurement fences, deletion exemptions, or a cross-domain mega-PR.

## Remaining blockers

Program B remains blocked on:

1. reconciliation of the issue body with the atomic ruling;
2. owner and external review of this dormant-base and two-context correction;
3. separate authorization, budget, and installation design for the App/control plane;
4. exact-SHA rule, caller, corpus, timing, RSS, cost, and amplification regeneration;
5. successful precursor review and protected-base promotion plus irreversible bootstrap disablement;
6. separate approval and disposable live proof of the temporary admission lock, this repository/API's `exempt` support, Mergify injection behavior, Merge Protections/Freeze, exact publisher binding, exclusions, all-path blocking, self-change reset, batch/queue races, retry classification/exhaustion, successful-ceremony timing and the pre-precursor abort threshold, latency, and API/quorum/audit behavior;
7. precursor and activation drafts prepared before admission lock; reviewed pre-precursor abort/restore; owner-approved rehearsal-derived successful-ceremony target, retry budgets, and pre-precursor abort threshold; explicit owner acceptance of post-precursor no recovery and the resulting unbounded terminal-tail ordinary-merge outage risk; and a successful pre-precursor final-state ceremony, terminal re-query, precursor merge under legacy authority, promotion/tombstone without later enforcement mutation, closed canary, and exact freeze evidence; and
8. an exact-manifest atomic activation/cutover reviewed under base-owned semantics.

Later subsystems have no implementation authority from this design.

## Spec self-review status

This correction pass checked scope, internal terminology, trust direction, context separation, rule-disposition safety, membership authority, accounting definitions, and historical/current separation. It does not claim owner approval, external review, generated inventories, control-plane authorization, precursor evidence, freeze evidence, or production readiness. Those remain explicit blockers above.
