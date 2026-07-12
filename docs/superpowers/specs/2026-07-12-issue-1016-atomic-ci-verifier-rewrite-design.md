# Issue #1016 Atomic CI Verifier Rewrite Design

## Decision and status

Issue #1016 will replace the central CI workflow-hygiene verifier atomically, but only after the complete replacement has been staged dormant on protected base. The protected `policy_base_sha` implementation is the sole semantic engine for the authority run. The atomic head may activate exactly the staged bytes, migrate callers and lifecycle-owned memberships, and delete the legacy path; it may not introduce or execute semantic implementation changes.

The user's goal is straightforward: CI must prove real safety from one owner, and an ordinary configuration change must not require Python or Python-test edits. Line count is secondary.

The governing sequence is approved: Program-A issue-owned deletions already landed; the precursor fixes the exact legacy deletion manifest; freeze follows promotion; and actual legacy deletion occurs only in atomic activation. The dormant-implementation correction, the two-context check design, and the separately governed control plane in this revision are not yet owner-approved. Publication and implementation require owner review and external adversarial review after the issue-body conflict is reconciled.

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
5. There is no legacy protocol adapter, old/new comparison, candidate-selected policy, dual authority, compatibility layer, or cleanup-later phase.
6. Missing, malformed, timed-out, crashed, cancelled, replayed, incomplete, unknown, neutral, or skipped authority outcomes do not authorize merge.
7. Every retained semantic rule has one owner, one typed contract, and independent clean/falsifying evidence or exact native-owner identity proof.
8. Each governed syntax has one restricted parser; retained adapters consume typed inputs and cannot reopen or reparse governed files.
9. An ordinary configured value has one lifecycle authority and requires no Python, test, fixture, or manual digest edit.
10. The final cutover leaves one active enforcement implementation and deletes the full legacy implementation, tests, helpers, aliases, and pins in the same head.

## Two-stage trust and cutover

### Stage 1: permanent precursor on protected base

After separate control-plane authorization, one precursor stages the complete replacement dormant: controller, classifiers, restricted parsers, typed snapshots, semantic registry, retained adapters, protocol validator, runner, materializer, corpus, manifest, bootstrap, focused tests, and an exact pending-activation manifest.

The legacy verifier remains the only repository semantic authority while the precursor is reviewed. Dormant conformance proves installability and expected failure behavior; it neither publishes merge authority nor compares old and new verdicts. The one-use bootstrap exception may authorize only this precursor after binding its complete reviewed byte set and protected base.

After merge, the external authority verifies the exact reviewed bytes at protected `policy_base_sha` and promotes them. It then writes the external monotonic tombstone and irreversibly disables all later bootstrap issuance and acceptance before running a closed canary against that post-tombstone, post-disable state. The covered surface freezes only after that canary passes. A failed canary remains fail-closed and blocked: the bootstrap exception cannot be reopened or reissued, and recovery requires a separately governed control-plane path. Any covered change invalidates caller, corpus, timing, RSS, cost, amplification, canary, and freeze evidence.

### Stage 2: atomic #1016 activation

The atomic head may only:

- match the base-owned pending-activation manifest exactly;
- keep every staged semantic, runner, protocol, parser, corpus, bootstrap, and test byte unchanged;
- switch the staged implementation from dormant to active;
- migrate direct and dynamic callers plus lifecycle-owned suite/lane/fingerprint membership;
- update provenance-bound generated artifacts deterministically; and
- delete all legacy implementation, tests, helpers, registrations, aliases, and obsolete pins.

Any semantic replacement edit or extra activation-surface change makes authority absent or failing. The base-owned staged implementation evaluates the atomic head, including its activation and deletion set.

### Future verifier versions

While Vn is sole authority, a separate PR stages Vn+1 dormant with its exact pending-activation manifest. Only after protected-base promotion and canary may an isolated activation PR, judged by base-owned Vn+1, activate Vn+1 and atomically delete Vn. No same-event comparison or candidate self-grading is permitted.

## Check contexts and Mergify authority

The repository uses Mergify rather than native `merge_group` as its normal merge authority. Live ruleset 14763242 gives Mergify App 10562 an always-bypass, and a bare check name does not bind its publisher.

Two external-App contexts have distinct purposes:

- `trusted-ci-verifier-feedback`: non-required ordinary-PR feedback only. It can never satisfy a ruleset or Mergify merge condition.
- `trusted-ci-verifier`: merge authority only. It is emitted for an exact Mergify proof head or a native `merge_group`, plus the single-use precursor bootstrap exception. It must be absent or failing on an ordinary PR head.

Protected Mergify configuration requires the App-qualified authority name, conceptually `check-success = @<external-app-slug>/trusted-ci-verifier`. The protected GitHub ruleset separately binds `trusted-ci-verifier` to the exact App integration ID. The actual installed App slug is an installation output and must not be a hardcoded placeholder in production configuration.

Authority is invalidated by mismatch or movement in base, head, parent, tree, constituent set, installation identity, launcher or artifact identity, ruleset epoch or digest, Mergify configuration, or activation manifest. Reuse, cancellation, replay, timeout, crash, malformed protocol, extra activation changes, stale API state, or terminal revalidation failure also invalidate it. Neutral and skipped conclusions are forbidden because GitHub accepts them as successful required conclusions.

### Exact authority tuple and bootstrap lifecycle

Every result is bound to one signed authority record containing repository ID; protected base repository and ref; external App integration and installation IDs; context name and purpose; ruleset ID, version, and digest; Mergify configuration digest and epoch; launcher SHA and artifact digest; exact `policy_base_sha`; proof `head_sha`; event kind and delivery ID; exact proof-head and branch identity; proof parent and tree identity; ordered constituent heads where applicable; activation-manifest digest; and staged-version identity. A bound tuple digest may compact the protocol record only when required identities remain explicit and protected validation reconstructs and verifies the complete tuple.

`policy_base_sha` is the exact protected base of the ordinary PR or proof head. Merge-base computation selects diff bytes only and is never policy or semantic authority. Immediately before terminal success, the App re-queries every identity, ancestry, object-existence, parent/tree, constituent, protected-ref, configuration, ruleset, launcher, installation, manifest, staged-version, and digest field. Movement, missing objects, changed ordering, staleness, ambiguity, or mismatch fails closed; earlier successful queries are not terminal proof.

For the precursor only, the App may mint one signed, expiring, event-specific bootstrap envelope bound to the complete tuple, protected `main`, the exact reviewed allowlist, and every reviewed content digest. It has no generic renewal, wildcard, derived-head reuse, or signer override. After merge, the App verifies the exact reviewed bytes at protected `policy_base_sha`, promotes those bytes, and records an external monotonic tombstone keyed to the precursor SHA once that SHA is an ancestor of protected `main`. Every later bootstrap issuance fails regardless of signer. Replay, reissue, datastore rollback, and restoration of an old App artifact must fail before canary or freeze.

## Separately approved control-plane prerequisite

This design does not authorize the external App, its installation, ruleset mutations, or operating budget. A separate approved control-plane design must define:

- authorization principals, quorum, and digest-bound approval;
- source-to-artifact attestation for launcher and installation verifier;
- append-only, rollback-resistant bootstrap tombstone and proof-reuse state;
- key custody, rotation, revocation, and recovery;
- audit retention and incident reconstruction; and
- ownership of ruleset and Mergify installation changes.

The App may authenticate, resolve and revalidate authority records, sandbox the base-owned runner, validate its envelope, and publish the appropriate context. It may not contain hidden repository policy, parse governed documents, interpret the semantic corpus, or choose candidate semantics.

## Components and dependency direction

The staged replacement consists of independently reviewable units:

- **Runner/materializer:** combines exact candidate documents with base-owned clean and falsifying cases under bounded resources; records provenance without exposing an answer key to candidate code.
- **Classifiers and parsers:** classify GitHub workflow/action YAML, TOML, Just, shell, and Mergify/Psych inputs, load each through one restricted owner, and produce immutable typed snapshots.
- **Semantic registry:** maps each stable rule ID to exactly one typed owner or retained native adapter; unknown, duplicate, or unowned IDs fail.
- **Protocol validator/reporter:** validates canonical structured results, completeness, identity, order, and termination; rendering never reinterprets policy.
- **Bootstrap:** installs exact locked parser/runtime artifacts hermetically and fails closed on unsupported, missing, corrupt, ambient, or mismatched inputs.
- **Activation manifest:** binds every staged byte, public entrypoint, caller disposition, lifecycle membership, generated output, legacy deletion, and allowed activation edit.

Dependency direction is:

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

Deterministic `just ci-verifier-budget-report` and a protected-base path/symbol inventory attribute full paths across all languages, including launcher seams, runner/protocol, facade, parsers, registry, adapters, bootstrap, measurement, focused tests, and generated executable code. Non-executable corpus and manifest exclusions are validated and reported separately. Hidden policy in data, generated output, retained owners, or another language is counted and rejected.

Non-Python repository code and external-App code are visible subtotals. App operations, maintenance, and hosting cost remain separate and visible. Semantic policy in the App is forbidden; permitted non-semantic conformance and control-plane code is still cost-accounted. This deterministic report is cutover evidence, never a permanent ordinary-PR line fence.

## Requirement-to-evidence matrix

| Requirement or risk | Required evidence |
| --- | --- |
| Immutable launch | Candidate workflow deletion cannot suppress either context; publisher/App binding and launcher artifact are exact. |
| Installation authority | Signed precursor envelope is allowlist/digest-bound; promoted bytes exactly equal reviewed bytes. |
| Proof-head merge authority | Feedback cannot authorize merge; wrong proof head, branch, parent/tree, constituent, context purpose, or App fails. |
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

1. Reconcile the issue body and record owner decisions; separately authorize the App/control plane.
2. Regenerate exact-main rule, caller, corpus, resource, cost, and attribution evidence.
3. Assign one implementer per declared file set; resolve internal findings and run cheap checks first.
4. Keep the precursor draft until exact-head feedback is green; request external/native review only after required exact-head CI or the approved equivalent is green.
5. Queue the precursor alone. After merge, verify and promote exact bytes at protected `policy_base_sha`, write the external monotonic tombstone and irreversibly disable later bootstrap issuance and acceptance, pass a closed canary against that state, and only then publish freeze evidence. Canary failure remains fail-closed and blocked; it cannot reopen or reissue the bootstrap exception and requires a separately governed control-plane recovery path.
6. Build the atomic activation with one implementer per file set; no covered semantic or corpus change is allowed.
7. Resolve internal findings and cheap checks, then obtain exact-head evidence before external/native review.
8. Verify ruleset, required reviewer, last-push approval, and review-thread requirements from `AGENTS.md`, then queue activation alone.

This sequence claims neither readiness nor approval.

## Rejected alternatives

- Candidate-head semantic execution, hidden finite cases, nonce-based honesty, and candidate-selected corpus: candidate self-grading.
- Old/new comparison or a legacy protocol adapter: dual authority and ambiguous failure ownership.
- A single check context for feedback and authority: ordinary PR feedback could be mistaken for merge proof.
- Bare required-check names: they do not bind the publishing App.
- Manual suite/TOML/fingerprint mirrors or tests that restate exact sets: multiple authorities.
- Missing-evidence deletion, expiry deletion, or silent corpus retention: unproved weakening.
- Generated hidden policy, compatibility layers, permanent measurement fences, deletion exemptions, or a cross-domain mega-PR.

## Remaining blockers

Program B remains blocked on:

1. reconciliation of the issue body with the atomic ruling;
2. owner and external review of this dormant-base and two-context correction;
3. separate authorization, budget, and installation design for the App/control plane;
4. exact-SHA rule, caller, corpus, timing, RSS, cost, and amplification regeneration;
5. successful precursor review and protected-base promotion;
6. freeze evidence; and
7. an exact-manifest atomic activation/cutover reviewed under base-owned semantics.

Later subsystems have no implementation authority from this design.

## Spec self-review status

This correction pass checked scope, internal terminology, trust direction, context separation, rule-disposition safety, membership authority, accounting definitions, and historical/current separation. It does not claim owner approval, external review, generated inventories, control-plane authorization, precursor evidence, freeze evidence, or production readiness. Those remain explicit blockers above.
