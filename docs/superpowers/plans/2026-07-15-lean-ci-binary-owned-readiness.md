<!--
Provenance
Approved: 2026-07-15
Source: /private/tmp/1016-lean-ci-implementation-plan.md
Source SHA-256: dda7e936f9070aaa550e4bb5f6f64f0e760947b12046fe47f4e87f4794615ad1
Governing decision: /private/tmp/1016-lean-ci-decision-packet-r2.md
Governing decision SHA-256: b2d6a5c9952078c695c2cff54352c1dbec8813974ca3469b6e6515730e3651db
External approval: Claude Code CLI (Anthropic), model claude-fable-5,
  /private/tmp/cmux-1016-lean-ci-plan-claude-r2-review.md, conclusion APPROVE.
The approved source plan was initially preserved except for adjudicated PR #1391 governance corrections: exact issue ownership for every implementation slice and completion of every applicable Task 8 family before Task 5. This durable copy now also carries the governed reconciliation below.
Governed remote-verification reconciliation: /private/tmp/1016-fast-probe-cold-warm-design.md,
SHA-256 30e30cb9fd8c125597593ff3e677a89761c4a3365c60c7cb4b533a380ac3f974,
inspected against main at 37e619b3fbd65fc041a05399ecf1750b8999567a.
-->

# Lean CI and Binary-Owned Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to execute one task in one isolated worktree. This reviewed master plan is copied into `docs/superpowers/plans/` only after approval; it is not implementation authorization.

**Goal:** Make CI small, visible evidence about the real Rust trading binary, remove CI as merge authority, and keep deployment and trading fail-closed inside the exact installed binary's `ops launch` path.

**Architecture:** One explicit trusted producer can seed one exact root or Backtester host target or build the ARM64 release binary; `root-artifact` is build-only and executes those exact bytes for positive and negative evidence. A separate explicit exact-SHA targeted probe runs one configured root or Backtester target. One content-addressed install path invokes a finite Rust pre-arm phase that alone can construct a one-use `LiveReadinessPermit`; Start consumes that value by move. Native human review governs merges, while the binary—not a CI result, cache, tag, or receipt—governs arming.

**Tech Stack:** Rust, Cargo nextest, cargo-zigbuild, pinned sccache-wrapped rustc, GitHub Actions on proven isolated managed runners, Mergify, systemd, TOML, AWS SDK for SSM.

## Global constraints

- Authoritative reconciliation base: `main` at `37e619b3fbd65fc041a05399ecf1750b8999567a`; refresh every implementation slice from then-current `main` after each merge.
- Governing durable design: `docs/superpowers/specs/2026-07-15-lean-ci-binary-owned-readiness-design.md`; the original decision packet is `/private/tmp/1016-lean-ci-decision-packet-r2.md`, SHA-256 `b2d6a5c9952078c695c2cff54352c1dbec8813974ca3469b6e6515730e3651db`.
- Preserve criteria M1-M3, B1-B3, L1-L3, S1-S3, and X1-X3. No slice may weaken a criterion to make its proof pass.
- #1016 owns architecture only. Every implementation branch requires an already assigned issue that owns its exact ledger row; Task 0 drafts missing ownership, and an operator must create and assign that issue before the branch starts. Never use a negated closing keyword next to an issue number.
- One implementer owns a file set at a time. Read-only audits and reviews may run in parallel.
- No trusted App, external publisher, service, database, signer, ceremony, persisted permit, compatibility adapter, result carry-forward, cache-as-proof, alternate installer, or fallback path.
- A launch log or receipt is audit-only. It is never read to authorize Start or a restart.
- Before zero-status cutover, current legacy exact-head gates remain mandatory. Use cheap non-compile local checks; publish a draft and use remote Rust verification rather than local compile-heavy Cargo commands.
- Until zero-status cutover, the required `gate` and `backtester-gate` reporters stay always present on every pull request. Do not path-filter, rename, delete, or weaken them.
- After zero-status cutover, heavy Rust evidence is explicit, exact-SHA, workspace-and-target-specific only and cannot authorize or veto merge. Native approval by node `U_kgDOEZMFhA`, stale-review dismissal, last-push approval, and human thread resolution remain mandatory.
- Root and Backtester remain separate workspaces. Routing either returns a reviewed finite explicit target set or `UNCLASSIFIED` with no command; no unknown path broadens to both workspaces, a whole workspace, or full CI.
- One pinned sccache-wrapped compiler path is mandatory. Cache is acceleration only; no direct/uncached retry, archive restore, prior result, carry-forward, sidecar, aggregate fallback, scheduler, or alternate public path may survive.
- No deploy, launch, trade, Mergify/ruleset mutation, or PR merge is implied by this plan.

## Dependency and concurrency map

| Lane | Depends on | Parallel work | Conflicting ownership |
|---|---|---|---|
| 0 Governance | approved plan | none | governance and issue metadata |
| 1 Explicit producer evidence | 0 | 2A, read-only predicate census | producer workflow, runner, compiler-wrapper, and config files |
| 2A Strict live target | 0 | 1, 3 design audit | `src/main.rs`, deploy config/docs |
| 2B Exact CLI negatives | 1, 2A | 3 | workflow and profile test evidence |
| 3 Immutable install | 0, interface agreement with 4 | 1, 2A | deploy installer/unit files |
| 4 In-process permit | 2A, 3 manifest interface | read-only deletion proofs | launch Rust files and systemd tests |
| 5 Operational replacement | 3, 4 green; every applicable Task 8 family complete | read-only audits | `ci.yml`, deploy path, `scripts/ci_provenance.py`, and `scripts/test_ci_provenance.py`; exclusive handoff to 6B |
| 6B Mechanical queue | 5 | read-only audits | queue scripts/config/tests plus exclusive ownership of `scripts/ci_provenance.py` and `scripts/test_ci_provenance.py` from fresh main; remove queue-CI expectation mirrors before handoff to 6A |
| 6A Advisory/queue config | 6B | read-only audits | AI files and `.mergify.yml`; if any Mergify expectation/preflight mirror survives 6B, 6A exclusively owns its coupled files |
| 7 Governed zero-status cutover | 5, 6B, 6A | none | `.mergify.yml`, any surviving coupled fixture, and the live required-status ruleset under an operator merge pause |
| 8 Runtime-invariant migrations | 2-4, complete ledger | independent semantic families; every applicable family must merge before Task 5 | Rust owners plus one old fence family |
| 9 Broad debt deletions | A9.2 preparation: 1 and 2B; A9.2 public cutover/deletion and other authority deletion: 7 plus relevant Task 8 family | non-conflicting preparation/deletion slices | CI/meta Python, workflow, routing, and measured harness files |
| 10 Measurement | 7 and representative 9 slices | read-only reporting | no production owner |

Task 5 is the common predecessor of the fixed `6B → 6A → 7` chain. Every applicable Task 8 family completes before Task 5; Task 8 cannot depend on or be deferred into that chain.

---

### Task 0: Approve governance and assign issue ownership

**Files/state:**
- Modify after plan approval: `AGENTS.md`, `.specify/memory/constitution.md`, `.pr_agent.toml`, affected `docs/ci/*.md`, and the live #1016 body.
- Create after approval: a durable two-board program ledger under `docs/ci/` if current `main` still lacks one.
- Live read-only inputs: GitHub issues, rulesets, Mergify configuration, required reviewer node.

**Produces:** an approved statement that CI is non-authoritative evidence, red `main` is accepted repository risk, exact-binary `ops launch` owns arming, advisory bots cannot block merges, and each Program-A deletion packet has a named issue.

- [ ] Re-query remote `main`, rulesets, required reviewer node, Mergify rules, and open CI-debt issues; record exact SHAs/IDs without mutating them.
- [ ] Amend #1016 and governance to supersede the trusted-App/precursor ceremony and to encode M1-M3 plus the no-fallback operational boundary.
- [ ] Create ledger rows and complete issue drafts for every implementation slice in Tasks 1, 2A, 2B, 3, 4, 5, 6A, 6B, 7, 8, and 9, with predicate/module IDs, callers, invariant, surviving owner, mutation/identity proof, owning issue, expected files, cost effect, reviewer, and later merged PR/SHA fields.
- [ ] Run `rg -n "trusted-ci-verifier|compatibility adapter|carry-forward|required exact-head CI" AGENTS.md .specify/memory/constitution.md .pr_agent.toml docs/ci` and adjudicate every hit against R2; expected result is only explicitly historical or pre-cutover language.
- [ ] Run `git diff --check` and the repository's targeted documentation/static checks. Obtain bounded internal adversarial review before publishing the docs-only draft.
- [ ] Stop if any implementation row lacks an exact assigned owning issue or if native review rules are absent. No implementation branch starts from an unapproved governance state.

### Task 1: Add one explicit trusted producer

**Files:**
- Create: one explicit producer workflow; do not create a second producer or compatibility entrypoint.
- Modify: `ci/github-actions-runners.toml`, `ci/rust-verification.toml`, the existing sccache setup/stats action, and existing TOML command/input owners only as needed for the single runner/compiler contract; do not add the producer to merge-required registries.
- Test/inspect: `.github/actionlint.yaml`, `tests/bolt_v3_prod_profile.rs`.

**Interface:** one `workflow_dispatch` producer accepts an exact trusted SHA and one mutually exclusive operation: `root-seed`, `backtester-seed`, or `root-artifact`. It emits target-specific cache metrics or the root artifact record `{head_sha, binary_sha256, overlay_ids, config_bundle_sha256[]}` and manifest for later operator selection. No output has a verdict or authority consumer.

- [ ] Keep the producer explicit-only: no `push`, `pull_request`, `merge_group`, tag, or schedule trigger. Validate the exact SHA and operation before a managed runner or cache-write role is selected.
- [ ] `root-seed` and `backtester-seed` each compile one exact configured host target in only its selected workspace and emit cache objects plus non-authoritative metrics. They emit no product artifact or test authority.
- [ ] `root-artifact` performs exactly one locked ARM64 release build, stages and hashes only the produced binary, executes only those bytes for configured positive and fail-closed evidence, and re-hashes them at the end. It runs no locked/full nextest, broad test, or targeted Cargo test suite.
- [ ] Enumerate tracked `config/profiles/*.overlay.toml` deterministically and compare the artifact evidence set with the repository set; omission, duplication, or unknown overlay fails. Invoke the exact binary for config generation/verification, malformed/unknown rejection, and the plain-`run` rejection.
- [ ] Root and Backtester select separate manifests, locks, target directories, commands, warmth identities, and evidence records. A producer operation cannot silently select the other workspace or an aggregate target.
- [ ] Preinstall and checksum/version-pin one sccache binary, always set it as `RUSTC_WRAPPER`, and fail before Cargo if the wrapper or configuration is invalid. Backend degradation remains inside the wrapper; there is no direct/uncached retry or archive/result fallback.
- [ ] Prove provider isolation and bounded capacity with at least five simultaneous producer requests: exactly one active writer, finite rejection/overflow, no cancellation of the accepted run, isolated ephemeral disks, and no repository scheduler/backlog.
- [ ] Negative search must find no automatic heavy trigger, hidden Cargo test, inherited result, archive reuse, carry-forward, binary sidecar, prior-result substitution, retry without the wrapper, or authorization consumer.
- [ ] Run locally: `just fmt-check`, `just deny`, `just ci-lint-workflow`, and bare actionlint through the existing public recipe; expected result is success without a Rust compile.
- [ ] Publish a draft with `just sandbox-safe-push`, run `just verify-remote`, and prove B1-B3 at the exact head. Record wall time and managed runner-minutes.
- [ ] Rollback before any consumer is deletion of the non-authoritative producer; current gates remain unchanged and no alternate producer is restored.

### Task 2A: Make live target verification strict

**Files:**
- Modify: `src/main.rs`, `config/deploy.toml`, `deploy/README.md`.
- Test: unit tests embedded in `src/main.rs` around `run_loaded_target_verify` and the `TargetVerify` stage.

**Interface:** advisory status may represent `NoTargetConfigured`; `ops launch` accepts only a matched, observable target and returns before SSM on absent, empty, name-tag-only, unobservable, or mismatched input.

- [ ] Replace the launch-path success arm for `TargetVerifyOutcome::NoTargetConfigured` with an error while preserving status-only rendering.
- [ ] Add discriminating tests for absent `deploy.toml`, empty `[target]`, name-tag-only target, unobservable host facts, and mismatched host facts. Each asserts SSM and Start runners were not called.
- [ ] Update TOML comments and operator docs in the same slice so no text says launch proceeds without a target.
- [ ] Run `just fmt-check`, targeted non-compile source checks, and `git diff --check`; then obtain exact-head remote Rust proof under legacy gates.
- [ ] Rollback restores neither optional live launch nor a second target path; a failed slice remains unmerged.

### Task 2B: Prove exact-binary secret and storage negatives

**Files:**
- Modify: the single explicit producer workflow's `root-artifact` evidence operation.
- Modify only if behavior evidence is missing: `src/main.rs`, `src/bolt_v3_secrets.rs`, `tests/bolt_v3_prod_profile.rs`.

- [ ] Run the exact binary's `secrets resolve` against valid generated production config with environment/shared credential sources and IMDS disabled. Require non-zero at `secrets-resolve`, field-context-only output, and no raw SSM path or value.
- [ ] Separately run exact-binary `ops prestart-check` with missing, unreadable, and insufficient storage catalogs. Require non-zero at prestart and no Start marker.
- [ ] Add Rust tests only for a demonstrated behavioral gap; reuse existing redaction/profile tests otherwise.
- [ ] Do not add another build or any Cargo test. Re-hash the binary after all cases and require equality with Task 1's `root-artifact` digest.
- [ ] Prove B2 remotely at the draft exact head. A cosmetic failure before the named stage is a failed proof, not success.

### Task 3: Install one content-addressed immutable artifact

**Files:**
- Create: `src/bolt_v3_release_manifest.rs` for strict manifest parsing and digest validation.
- Modify: `src/lib.rs`, `src/main.rs`, `deploy/install.sh`, `deploy/install-layout.env`, `deploy/systemd/bolt-v2.service.in`, generated `deploy/systemd/bolt-v2.service`, `scripts/render_install_unit.py`, `deploy/README.md`.
- Test: `tests/deploy_systemd.rs` and module tests in `src/bolt_v3_release_manifest.rs`; regenerate the unit with `python3 scripts/render_install_unit.py` and require byte equality with the committed unit.

**Proposed interface:**
```rust
pub(crate) struct SelectedArtifactManifest {
    pub(crate) main_sha: String,
    pub(crate) artifact_sha256: String,
    pub(crate) config_bundle_sha256: String,
}

pub(crate) fn verify_selected_executable(
    manifest: &SelectedArtifactManifest,
    current_exe: &std::path::Path,
) -> Result<(), ReleaseManifestError>;
```

- [ ] Parse with unknown-field rejection and validate lowercase full-length Git/SHA-256 fields; missing or malformed fields fail.
- [ ] Stage bytes under a digest-derived directory, verify before atomic placement, make the installed file non-writable by the service user, and render systemd `ExecStart` with that exact path.
- [ ] Reject a mutable `/opt/bolt-v2/bolt-v2` target, wrong digest, wrong main SHA, changed bytes, alternate installer input, and mutable-copy launch.
- [ ] Keep artifact selection and transport mechanical; do not add signature, service, publisher, database, cache proof, or second install path.
- [ ] Run install/render static tests, `tests/deploy_systemd.rs` remotely, and verify the generated unit is byte-identical to its template rendering.
- [ ] Candidate installation remains rehearsal-only until Task 5; deploy/trading stays paused during rehearsal.

### Task 4: Put the sole arming boundary inside Rust

**Files:**
- Create: `src/bolt_v3_live_readiness.rs`.
- Modify: `src/lib.rs`, `src/main.rs`, `src/bolt_v3_operator_artifacts.rs` only to mark persisted launch identity audit-only, and `tests/deploy_systemd.rs`.
- Test: focused module tests plus existing launch-chain tests in `src/main.rs`.

**Proposed interface:**
```rust
pub(crate) struct LiveReadinessInput<'a> {
    pub(crate) manifest: &'a SelectedArtifactManifest,
    pub(crate) config_root: &'a std::path::Path,
    pub(crate) profile: &'a str,
    pub(crate) cancellation_requested: &'a dyn Fn() -> bool,
}

pub(crate) struct LiveReadinessPermit {
    runtime: BoltV3LiveNodeRuntime,
    loaded: LoadedBoltV3Config,
    _sealed: (),
}

pub(crate) fn run_live_readiness(
    input: LiveReadinessInput<'_>,
) -> Result<LiveReadinessPermit, Box<dyn std::error::Error>>;

fn start_ready_node(
    permit: LiveReadinessPermit,
) -> Result<(), Box<dyn std::error::Error>>;
```

- [ ] Move the finite pre-arm orchestration behind `run_live_readiness`: executable/manifest, config-bundle, strict target, SSM, storage/prestart, reference health, then shared-runtime construction.
- [ ] Keep the permit's fields and constructor private; do not implement `Clone`, `Copy`, serialization, or persistence. The only Start function consumes it by value.
- [ ] Place the final cancellation check before construction and move the permit immediately into Start with no await, file publication, or fallible authorization step between them.
- [ ] Add counter-mutations proving Start is unreachable when each stage fails, when cancellation occurs, when an audit receipt exists without a permit, and when a restart tries to reuse prior state.
- [ ] Prove systemd restart reruns every phase and obtains a fresh in-memory permit. Post-Start cancellation is runtime termination, not a readiness result.
- [ ] Use cheap local formatting/static checks and exact-head remote Rust tests. Stop if source structure cannot enforce a single Start API without a compatibility path.

### Task 5: Replace operational authority atomically

**Files:**
- Modify: `.github/workflows/ci.yml`, `ci/github-actions-runners.toml`, `deploy/install.sh`, `deploy/systemd/bolt-v2.service.in`, generated unit, `deploy/README.md`.
- Delete or narrow after caller proof: same-SHA/tag/prior-artifact sections in `scripts/ci_provenance.py` and their exact tests in `scripts/test_ci_provenance.py`.
- Shared-file ownership: Task 5 exclusively owns `scripts/ci_provenance.py` and `scripts/test_ci_provenance.py` until merge; Task 6B starts from that fresh `main`, and Task 6A starts only after Task 6B's handoff.

- [ ] Task 5 must not start until Tasks 3-4 and every applicable Board-B Task 8 migration are complete on `main` under legacy gates.
- [ ] From fresh main, prove Tasks 3-4 candidate behavior and every applicable Board-B Task 8 migration while legacy gates still exist and deploy/trading is paused.
- [ ] In one owning-issue slice, activate the immutable installer/systemd path and delete or hard-disable tag deploy, `same-sha-main-evidence`, prior-main artifact selection, and manual copy without manifest binding.
- [ ] Run negative tests for tag, prior run, same SHA, mutable copy, wrong path/digest/config bundle, audit-receipt substitution, and restart inheritance.
- [ ] Drill a failed cutover with deploy/trading paused and recover by forward fix; prove no retired deploy or trading route is restored.
- [ ] Obtain internal adversarial review specifically for S1/L2/L3, then exact-head legacy-gate proof and required native review.
- [ ] Operational rollback is deploy/trading pause or forward fix. Never restore the retired route.

### Task 6A: Make advisory review and Mergify genuinely non-blocking/single-PR

**Files:**
- Modify as required by direct inspection: `.github/workflows/claude-code-review.yml`, `.github/workflows/ai-review-glm-pr-agent.yml`, `.github/workflows/ai-review-kimi-cli.yml`, `ci/ai-review.toml`, `scripts/ai_review_deliverables.py`, their focused tests, `.mergify.yml`, and `.pr_agent.toml` where mirrored behavior changes.
- Task 6B removes the queue-CI `MERGIFY_CONFIG_EXPECTATIONS` and matching preflight fixtures before this slice. If any Mergify mirror remains, Task 6A exclusively owns and updates `scripts/ci_provenance.py`, `scripts/test_ci_provenance.py`, and `scripts/test_merge_queue_preflight.py` with its `.mergify.yml` batch-size edit; Task 7 likewise owns any surviving coupled update with predicate deletion.

- [ ] Remove inline-comment tooling and line-specific prompting from every retained advisory reviewer; permit only top-level comments or non-required summaries and no `REQUEST_CHANGES` review state.
- [ ] Set `batch_size: 1` explicitly in default and hotfix Mergify rules. Keep native human approval and thread rules.
- [ ] Keep all four `check-success` predicates in both Mergify rules through Task 6A's merge. They are the binding pre-cutover CI authority because live Mergify has an `always` bypass on the CI-gates ruleset; Task 7 alone owns their removal.
- [ ] Prove with disposable/no-merge PRs that two queued PRs receive separate contexts and an advisory failure/comment cannot change mergeability.
- [ ] Keep legacy status requirements active during this PR. Rollback is a code/config revert before Task 7, not a new bot bypass.

### Task 6B: Reduce `just merge-queue` to mechanical admission

**Files:**
- Modify: `justfile`, `scripts/merge_queue_operator.py`, `scripts/merge_queue_preflight.py`, `scripts/test_merge_queue_operator.py`, `scripts/test_merge_queue_preflight.py`, `ci/rust-verification.toml`, operator docs.
- Modify/delete exact registry consumers: `scripts/ci_provenance.py`, `scripts/test_ci_provenance.py`.
- Modify: `docs/ci/merge-queue-preflight-contract.md` so it documents mechanical admission rather than CI verdicts.
- Shared-file ownership: start from fresh `main` after Task 5 merges, then exclusively own `scripts/ci_provenance.py` and `scripts/test_ci_provenance.py`; Task 6A starts only after this slice merges and may not run concurrently.

**Retained checks:** exact PR/head/base identity, native approval and human-thread state, merge conflicts, and Mergify routing only.

- [ ] Delete required/all-check polling, CI verdict aggregation, required-check workflow maps, the queue-CI `MERGIFY_CONFIG_EXPECTATIONS` mirrors and matching preflight fixtures, source-fence/verifier profiles and execution, and CI-dependent queue verdict tests.
- [ ] Through public `just merge-queue`, prove failed, missing, skipped, cancelled, and unavailable advisory checks produce the same queue decision as green checks.
- [ ] Keep remote SHA verification and required-reviewer identity checks; do not duplicate Mergify or ruleset verdict policy.
- [ ] Run focused Python suites once plus `just source-fence-static` while it still exists. Keep legacy GitHub statuses active until Task 7.

### Task 7: Remove Mergify predicates and live required statuses in one governed cutover

**Files/state:** after Tasks 5, 6B, and 6A are merged under legacy governance, the exact repository cutover commit modifies `.mergify.yml` and any surviving coupled expectation/preflight fixtures together; the operator cutover separately modifies the live GitHub required-status ruleset while a controlled merge pause guards the cross-system boundary.

- [ ] Re-query remote main, native code-owner approval, stale dismissal, last-push approval, human thread resolution, Mergify config, and the public queue entrypoint.
- [ ] Re-prove M1-M3, B1-B3, L1-L3, and S1-S3 on the same reviewed head; pause if any evidence is stale.
- [ ] Prepare and review an exact cutover commit under the legacy gates that removes all four `check-success` predicates from both Mergify rules and updates or removes any surviving coupled expectation/preflight fixtures.
- [ ] Use a controlled operator merge pause rather than claiming cross-system atomicity. With the reviewed cutover commit ready but not queued, block every other merge, then use explicit operator authorization to remove `actionlint`, `gate`, `backtester-gate`, and `host-health` from the live required-status ruleset while the old Mergify predicates still remain the binding queue authority.
- [ ] Admit only the exact cutover commit and queue it against the still-old base Mergify rules, so those four predicates bind its merge; that commit removes the predicates from authoritative `main`. No other merge may cross the repository/live-state boundary during this sequence.
- [ ] Fail closed until both removals are verified; never open an early zero-gate window. Confirm both Mergify rules contain no `check-success` predicate and the required-status list is empty before lifting the operator merge pause.
- [ ] Verify failed/missing advisory results cannot veto a disposable merge, while a missing native approval or unresolved human thread still blocks.
- [ ] Call a queue pause an operator pause unless a named live rule proves GitHub enforcement. Rollback is a merge pause and forward fix; broad CI gates are not restored.

### Task 8: Migrate genuine invariants by semantic owner

Task 8 begins only after Tasks 2-4 and the complete predicate ledger. Every applicable family must merge under legacy gates before Task 5 starts; no family may bypass, defer, or depend on Task 5, 6B, 6A, or 7.

**Ledger families and existing evidence:**
- Strategy/shared admission: `scripts/verify_bolt_v3_strategy_policy_fence.py`, `scripts/verify_bolt_v3_no_exit_market_command.py`, Rust tests under `src/bolt_v3_live_node/tests/`.
- Poison/fail-closed state: `scripts/verify_bolt_v3_poison_lock_fence.py`, relevant Rust tests under `src/bolt_v3_live_node/tests/`, `src/bolt_v3_iv/`, `src/nt_runtime_capture.rs`.
- SSM/config/redaction: `src/bolt_v3_secrets.rs`, `tests/bolt_v3_prod_profile.rs`.
- Provider boundary/reference health: `scripts/verify_bolt_v3_boundary_evidence.py` and its tests/registry consumers.
- Packaged systemd: `scripts/verify_install_unit_generated.py`, `tests/deploy_systemd.rs`.

- [ ] Partition every remaining Python/source-fence predicate into bounded packets with one semantic owner and one assigned issue; no unknown row advances.
- [ ] For each genuine runtime invariant, plant a discriminating Rust behavior/integration mutation, prove the retained Rust test catches it, then delete the old lexical/shape predicate in a separate or explicitly dependent slice.
- [ ] Preserve non-deferrable WebSocket registry completeness, production decoding, reference-health degradation, shared admission, poison fail-closed behavior, SSM-only redaction, and systemd restart readiness.
- [ ] Delete broad token/literal/implementation-shape checks instead of translating them to Rust.
- [ ] Independent families may run in parallel only when their Rust and old-fence file sets do not overlap. Each slice receives internal adversarial review and its own before/after cost record.

### Task 9: Delete non-authoritative CI/Python debt in bounded waves

**Candidate files by assigned wave:**
- A9.2 targeted-probe/archive closure: `.github/workflows/rust-probe.yml`, `.github/workflows/debug-test.yml`, `.github/workflows/backtester-ci.yml`, `.github/scripts/run-rust-probe.sh`, `scripts/rust_verification.py`, archive/fingerprint/cache/sidecar scripts and tests, root and Backtester TOML/justfile owners, and only the root/Backtester test-harness files required by measured target decomposition.
- `.github/workflows/merge-readiness-finalizer.yml`, `.github/workflows/coverage-enforcer.yml`, `.github/workflows/dispatch-ci-cancel.yml`, `.github/workflows/ai-review-coding-plan-smoke.yml`.
- `scripts/verify_ci_workflow_hygiene.py`, `scripts/test_verify_ci_workflow_hygiene.py`, `scripts/merge_readiness.py`, `scripts/test_merge_readiness.py`, `scripts/coverage_enforcer.py`, `scripts/test_coverage_enforcer.py`, `scripts/cancel_obsolete_dispatch_runs.py`, `scripts/test_cancel_obsolete_dispatch_runs.py`, `scripts/lane_governor.py`, `scripts/test_lane_governor.py`, `scripts/verify_lane_governance.py`, `scripts/test_verify_lane_governance.py`, and non-runtime residue in `scripts/run_fences.py` plus its tests.

- [ ] In an issue-named A9.2 preparation slice under unchanged pre-Task-7 gates, measure clean/warm compile latency and source/module fan-in for every routed root and Backtester target. Split only targets that miss the rehearsed budget; preserve exact test semantics and one inventory, with no aggregate compatibility harness. The measured 75 Backtester modules/40,730 lines, 90,894 root lines, and 131,624 combined lines are context only.
- [ ] Reuse the existing suggestion/dispatch wrapper to route every changed path to a reviewed finite set of explicit `(workspace, build class, Cargo target, selector)` tuples or `UNCLASSIFIED`. A finite result prints one separately authorized command per target; `UNCLASSIFIED` prints none and never broadens to both workspaces, a whole workspace, or full CI.
- [ ] Before public activation, dispatch at least five simultaneous candidate probes per workspace and prove isolated ephemeral disks, the selected finite provider cap/overflow behavior, and non-cancellation. Do not add a repository scheduler or unbounded backlog.
- [ ] After Task 7 and caller migration, replace the legacy Rust Probe public contract and Debug Test atomically with one exact-SHA, one-workspace, one-target, read-only probe. Then delete the complete root and Backtester archive/fingerprint/reuse/carry-forward/sidecar/compile-retry/fallback closure; no dual accepted debug path, dormant mode, or compatibility alias survives.
- [ ] Delete automatic root and Backtester compile lanes on pull requests, merge groups, and `main` pushes only after Task 7. Backtester-only and documentation-only changes must consume zero root Cargo minutes; root binary-only changes must consume zero Backtester Cargo minutes; proven root path-dependency changes select explicit targets in both.
- [ ] Create one PR per assigned issue or named non-conflicting slice; exact live callers and imports must be zero or removed in that slice.
- [ ] Delete dynamic classifiers, gate names, merge-readiness/finalizer, provenance/carry-forward, archive/fingerprint/cache reuse, duplicated Rust execution, CI self-governance, and obsolete reporting with no named consumer.
- [ ] Make any surviving non-heavy Backtester, host-health, actionlint, fmt/clippy, dependency, AI, coverage, flaky, storage, and cost evidence advisory/manual/scheduled with a named consumer; do not add an aggregator.
- [ ] For each slice run targeted Python/static tests that remain, `git diff --check`, negative residue search, internal adversarial review, and exact-head advisory evidence. Record cost, automatic frequency, cold/warm latency, routing, bounded parallelism/disk, and failure-output outcomes. Record file, line, branch, and module counts only as diagnostic telemetry.
- [ ] Revert only a non-authority deletion slice if its surviving invariant proof fails. Never reintroduce a fallback result, queue veto, tag deploy, or mutable install path.

### Task 10: Measure and close the program

**Files:** update the durable program ledger and CI cost/governance documentation only.

- [ ] Compare equivalent PR, merge-group, `main`-push, explicit probe/producer, scheduled, and deploy events with the frozen baseline: workflows/jobs, wall time, managed runner-minutes, duplicate checks per SHA, failure causes, and binary/live proof coverage. Heavy automatic frequency must be zero.
- [ ] Demonstrate one public targeted-probe graph, one explicit producer graph, one immutable install/launch graph, zero required CI statuses, and no advisory authorization edge.
- [ ] Record a diff-inventory attestation that the 36,704–38,043-line trusted-control-plane rehearsal was not merged, except for independently reviewed runtime slices that stand on their own evidence.
- [ ] Record exact merged PRs and main SHAs for every ledger row. Keep broader issues open when accepted scope remains.
- [ ] Obtain one final architecture review against all 15 criteria and required native approval for documentation. Success is lower cost/latency and less policy code with no lost trading invariant.

## Review and publication protocol for every implementation slice

1. Confirm the exact ledger row has an assigned owning issue, then create a fresh worktree from authoritative `main`; report base SHA and intended file ownership.
2. One bounded implementer; no nearby cleanup.
3. Cheap local non-compile checks only.
4. Separate internal adversarial reviewer maps every requirement to evidence and reports exact base/head, changed files, remaining scope, and cleanliness.
5. Resolve findings before publication. Push draft with `just sandbox-safe-push`; use `just verify-remote` for exact-head Rust feedback while legacy gates apply.
6. External/native review only after the applicable exact-head evidence is adjudicated and threads are resolved. Before Task 7, use the required full gates; after Task 7, use the proportionate explicit exact-target evidence and keep native review as authority.
7. Merge only through `just merge-queue`; after Task 6B it is mechanical admission, not a CI verdict engine.

## Criterion traceability

| ID | Plan obligation | Owning tasks |
|---|---|---|
| M1 | Zero required CI statuses and no advisory veto | 0, 6A, 6B, 7 |
| M2 | Native controls plus one-PR Mergify queues | 0, 6A, 7 |
| M3 | Accepted red-main risk with separate fail-closed live authority | 0, 7 |
| B1 | Explicit producer; target-specific seeds; build-only locked ARM64 `root-artifact` with no Cargo tests | 1 |
| B2 | Exact-file overlay, secret, storage, and fail-closed evidence against the one produced artifact | 1, 2B |
| B3 | Visible evidence with no merge/install/live authority | 1, 6B, 7 |
| L1 | Exact commit/digests, strict target, ordered pre-arm checks | 2A, 3, 4 |
| L2 | One immutable installer and permit-consuming Start path | 3, 4, 5 |
| L3 | Finite in-process one-use permit with no substitute | 4, 5 |
| S1 | One targeted-probe path, one producer path, and one install/launch path | 1, 3-5, 9 |
| S2 | One policy owner and mechanical-only queue admission | 6A, 6B, 7, 9 |
| S3 | Complete deletion boundary without re-encoding debt | 8, 9 |
| X1 | Safe ordering through operational and merge cutovers | 0, 5-7 |
| X2 | Complete issue-owned predicate/deletion ledger | 0, 8, 9 |
| X3 | No fallback rollback or rehearsal merge; measured outcome | 5, 7, 10 |

## Plan-level stop conditions

- Any failed internal resolution criterion returns the design to the owner; no third correction loop.
- Any missing exact implementation issue assignment, overlapping implementer file set, stale main, absent native review control, ambiguous artifact identity, `UNCLASSIFIED` route, second compiler path, or second public probe/producer/operational path stops that slice.
- Any negative case that reaches Start, logs credential material/raw SSM paths, or accepts a persisted receipt as authority stops all operational cutover work.
- Any cutover failure pauses merge, deploy, or trading as appropriate and receives a forward fix. It does not restore the rejected CI maze or legacy deploy path.
