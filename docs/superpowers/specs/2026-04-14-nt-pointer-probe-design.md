# NT Pointer Probe Design

## Status

Draft design for review.

## Scope

This spec defines a fail-closed process for keeping `bolt-v2` aligned with upstream Nautilus Trader (NT) without auto-landing incompatible bumps.

This design includes:

- a scheduled NT probe workflow for both `develop` and tagged-release lanes
- a hand-curated seam registry describing where Bolt semantics overlap NT semantics
- an evidence contract for when an NT bump is considered safe enough to open a draft PR
- fail-closed behavior for ambiguous or insufficiently-proven bumps
- an explicit out-of-band external adversarial review step owned by the human operator

This design does **not** include:

- automatically merging NT bumps
- changing Bolt runtime behavior
- replacing existing CI, Dependabot, or release workflows
- outsourcing gating decisions to commit-message heuristics or model inference

## Problem

`bolt-v2` pins many NT crates to one git revision in [`Cargo.toml`](/Users/spson/Projects/Claude/bolt-v2/Cargo.toml:19). NT moves quickly. If Bolt does not revisit that pointer regularly, drift accumulates and the eventual catch-up becomes larger and riskier.

The main risk is not that NT is low-quality upstream. The main risk is that Bolt may have adapted to older NT APIs or semantics, and a correct upstream NT change may break a Bolt-owned seam.

Examples of that risk class:

- NT changes execution or reconciliation behavior and Bolt has assumptions downstream
- NT changes live config defaults or validation and Bolt materialization/bootstrap becomes stale
- NT changes subscription ownership or custom-data flow and Bolt actors or mocks no longer match
- NT changes persistence or event semantics and Bolt lake or normalized-sink contracts drift

Simple “CI went green” is necessary but not sufficient if the bump changes a semantic overlap area that lacks a named proof.

## Design Goals

1. Never auto-land a Bolt-breaking NT bump.
2. Keep NT drift small by probing regularly.
3. Prefer false negatives over false positives.
4. Make the decision process explicit and reviewable.
5. Separate authoritative Bolt-owned risk knowledge from advisory heuristics.
6. Allow automation to open a draft PR when evidence is sufficient.
7. Require explicit human review before any merge.

## Non-Goals

1. Predicting every possible semantic regression from upstream metadata alone.
2. Trusting LLM summarization or file-path heuristics as a primary safety mechanism.
3. Automatically classifying all future NT changes correctly with zero maintenance.
4. Treating upstream `develop` as merge-equivalent to tagged releases.

## Existing Baseline

The repository already has strong mechanical checks:

- [`fmt`, `deny`, `clippy`, `test`, and `build` in CI](/Users/spson/Projects/Claude/bolt-v2/.github/workflows/ci.yml)
- [weekly Cargo and Actions Dependabot updates](/Users/spson/Projects/Claude/bolt-v2/.github/dependabot.yml)
- [repo-owned smoke/build verification](/Users/spson/Projects/Claude/bolt-v2/tests/verify_build.sh)

This design builds on that baseline. It does not replace it.

## Decision Model

The NT pointer probe must use two layers of classification:

1. **Authoritative seam registry**
2. **Advisory upstream diff inference**

The seam registry is the true gate. Advisory inference can only make the system stricter. It must never reduce required evidence, classify an unmapped change as safe, or suppress a registry-required canary.

This hierarchy is required because commit messages and file paths are too weak to detect all Bolt/NT semantic overlap. A false negative is the exact failure we are trying to avoid.

## Seam Matching Function

The design must define mechanically what it means for an NT bump to touch a seam.

For every changed upstream NT path in the resolved diff, the probe must classify that path into exactly one of these buckets:

1. one or more registered seams
2. an explicit safe list entry
3. ambiguous

That classification must come from the seam registry itself, not from advisory inference.

The seam registry therefore must include:

- upstream NT path-prefix mappings for each seam
- an explicit safe list for paths proven non-overlapping with Bolt semantics

The safe list must stay narrow. It is only for clearly non-overlapping areas such as:

- upstream docs
- examples
- tests
- unused adapters Bolt does not compile against

Shared NT crates and shared vocabulary paths such as `nautilus-model`, `nautilus-common`, `nautilus-core`, `nautilus-live`, and `nautilus-network` must not be safe-listed broadly.

Safe-list governance must be stricter than ordinary seam edits because the safe list is the highest-leverage bypass path in the system.

Each safe-list entry must include:

- the exact path or prefix being classified
- the non-overlap proof
- the Bolt configuration condition that makes it safe
- the reviewer or owner who approved it
- an expiry or revalidation deadline

Additional safe-list rules:

- safe-list additions must require the same owner-group review as registry changes, and may require stricter review
- safe-list changes must not be bundled with unrelated code changes
- if a safe-list condition no longer holds for the current Bolt codebase, the entry is invalid and the path becomes ambiguous
- path-prefix safe-list entries inside shared NT crates are forbidden; only exact-path entries may be considered there, and only with heightened review

Fail-closed rules for matching:

- any changed NT path not matched to a seam or safe list is ambiguous
- any ambiguous path fails the probe
- advisory inference may add more seams to run or escalate to ambiguous
- advisory inference may never move a path from seam or ambiguous to safe

## Baseline Preconditions

The following are baseline preconditions, not seams:

- pinned NT revision update
- lockfile refresh
- full CI-equivalent mechanical suite
- build verification and repo-owned smoke checks

These are required for every probe run regardless of touched seams.

The required probe environment must also be concrete:

- it must be a named CI-managed runner image, container image, or deploy-equivalent environment
- it must not be an arbitrary developer laptop
- its identity must be recorded in the evidence artifact

## Probe Lanes

The system must maintain two separate NT probe lanes.

### Lane 1: `develop`

Purpose:

- detect Bolt/NT incompatibility early
- keep upgrade deltas small
- surface draft PRs quickly when evidence is strong enough

Behavior:

- scheduled probe against upstream NT `develop`
- resolve `develop` to an immutable full SHA before any other step
- if all required evidence passes, automation may open a draft PR
- if evidence is incomplete or ambiguous, the probe reports failure and opens no PR

### Lane 2: tagged releases

Purpose:

- provide a calmer upstream target
- give the team a more conservative lane for merge candidates

Behavior:

- scheduled or manually-triggered probe against the newest NT tagged release
- resolve the release tag to an immutable full SHA before any other step
- record both the tag name and the resolved SHA in probe output
- require a tag soak window before draft-PR creation; the default soak window should be 7 days unless the registry owner explicitly tightens it further
- measure soak as consecutive days where the same tag name resolved to the same SHA in probe observations; any SHA move resets the clock
- same gating model as `develop`
- also draft-PR only, never auto-merge

Security exception path:

- an urgent security-tagged NT release may bypass the soak wait for draft-PR creation only
- it must still satisfy the full evidence contract
- it must still remain draft-only and subject to the external adversarial review gate before merge

## Lane Precedence

The two lanes do not have equal merge authority.

- the `develop` lane is an early-warning lane
- the tagged-release lane is the merge-candidate lane

If the two lanes disagree, the tagged-release lane result takes precedence for merge decisions.

If a tagged-release probe supersedes an open `develop`-lane draft PR, the `develop`-lane PR must be marked superseded or closed automatically.

If a `develop`-lane target includes a tagged-release SHA that is still inside its soak window, the `develop`-lane PR must inherit that soak restriction for mergeability.

## Terminal Action

The maximum automated action is:

- open a **draft PR**

The system must never:

- auto-merge
- auto-enable merge queues
- rebase and push over a human-owned branch
- silently update `main`

## Seam Registry

Bolt must maintain a hand-curated registry of semantic overlap seams. Each seam must define:

- seam name
- why it is semantically risky
- Bolt-owned NT usage it owns
- upstream NT path-prefix mappings that force this seam
- required coverage classes for this seam
- Bolt-owned canary tests or probes required for proof
- canary coverage class for each required canary
- escalation behavior if an NT bump appears to touch the seam ambiguously

The initial conservative seam set should include at least:

1. **Polymarket execution, fee, and reconciliation**
   - Risk: commission math, reconciliation logic, fill parsing, cancel/fill races, position identifiers
   - Likely evidence: Polymarket bootstrap, execution-related tests, targeted reconciliation canaries

2. **Live node config, defaults, validation, and bootstrap**
   - Risk: NT config shape or default drift breaking Bolt materialization or startup assumptions
   - Likely evidence: config parsing, render-live-config tests, live-node bootstrap tests

3. **Subscription and custom-data semantics**
   - Risk: ownership and dispatch changes in data-client traits, actor subscription flow, custom-data delivery
   - Likely evidence: compile-time API compatibility, reference actor tests, reference pipeline tests

4. **Reference pipeline behavior**
   - Risk: Chainlink/custom-data routing, venue subscription behavior, fused-price assumptions
   - Likely evidence: reference actor and pipeline tests

5. **Normalized sink, persistence, and lake contract**
   - Risk: event shape drift, message semantics, persistence contract drift, lake conversion assumptions
   - Likely evidence: normalized sink tests, lake batch tests, persistence smoke checks

6. **Network / TLS / reconnect transport**
   - Risk: TLS, websocket, retry, reconnect, DNS, and transport-semantic drift in live network paths
   - Likely evidence: controlled transport probes and reconnect-focused integration canaries

7. **Toolchain / MSRV / platform build contract**
   - Risk: upstream MSRV or toolchain drift breaking Bolt’s pinned toolchain or deploy/build contract
   - Likely evidence: build in the repo-pinned toolchain and deploy-equivalent environment

8. **NT shared type contract / cross-crate vocabulary**
   - Risk: shared `nautilus-model` / `nautilus-common` / `nautilus-core` type or schema drift breaking multiple Bolt seams at once
   - Likely evidence: compile-time API guards, serialization or schema compatibility checks, cross-version persistence readback

9. **Time / ordering / timer semantics**
   - Risk: timer, scheduling, clock, or event-ordering drift breaking Bolt logic that depends on thresholds, quiet periods, or sequencing
   - Likely evidence: timer- and ordering-sensitive canaries for reference and runtime flows

### Initial Registry Clarification

The prior open concern around NT issue `#3806` (“Polymarket data client auto-subscribe vs. strategy-driven subscriptions”) should **not** remain an unresolved seam by itself.

Current evidence in Bolt suggests:

- Bolt builds explicit Polymarket data-client filters in [`src/clients/polymarket.rs`](/Users/spson/Projects/Claude/bolt-v2/src/clients/polymarket.rs:142)
- Bolt-owned actors drive subscriptions explicitly, for example in [`src/platform/reference_actor.rs`](/Users/spson/Projects/Claude/bolt-v2/src/platform/reference_actor.rs:237)

Therefore this concern should be covered under the broader “subscription and custom-data semantics” seam rather than treated as a separate unresolved blocker.

The seam entry must still define the concrete behavior Bolt relies on and the canary that proves it. This clarification is not, by itself, sufficient evidence.

## Registry Construction And Completeness

Before this probe can be trusted, the project must perform a one-time registry construction audit.

That audit must:

1. enumerate every direct NT dependency pinned in `Cargo.toml`
2. enumerate every Bolt source use of `nautilus_*`
3. map each dependency and use site to a seam owner
4. fail if any dependency or use site has no seam owner
5. verify that each owning seam has an upstream path-prefix mapping that would actually classify changes to the relevant NT crate or path
6. verify that each registered upstream path-prefix matches at least one existing path in the target NT source tree
7. inventory canary gaps for every seam and mark the registry incomplete until all required seam canaries are real, not stubs

After activation, every probe run must re-check registry completeness against the current Bolt codebase.

Fail-closed rule:

- if Bolt currently uses any `nautilus_*` symbol, crate, or path with no seam owner, the probe fails as a registry-gap failure
- if a seam's registered upstream path-prefix no longer matches any existing NT path, the probe fails as a registry-gap failure
- if a Bolt-owned NT usage is mapped to a seam whose path-prefixes would not classify changes to the corresponding NT crate or path, the probe fails as a registry-gap failure

Registry ownership must also be explicit:

- the seam registry must have a designated owner group
- that owner group must be mechanically required for registry changes
- the mechanism must be concrete, for example CODEOWNERS plus required review, not a policy note
- until such ownership exists, the probe must not be allowed to open draft PRs automatically

## Evidence Contract

A probe is only allowed to open a draft PR if **all** of the following are true:

1. The upstream lane target is resolved to an immutable full SHA.
2. The NT pointer is updated in a fresh isolated probe branch or worktree.
3. The lockfile refresh succeeds and the resulting lockfile is tied to that exact NT SHA.
4. The full repo CI-equivalent suite passes in the pinned toolchain and required probe environment.
5. Every changed upstream NT path is classified as seam-owned, safe-listed, or ambiguous.
6. Every required seam has passing evidence.
7. Every required canary exists, is executed, and produces assertion results.
8. Every touched seam has all of its required coverage classes satisfied by passing canaries.
9. Every required canary has an explicit coverage class recorded in the registry and in the probe evidence.
10. The Bolt-side `nautilus_*` usage inventory for the probe run is recorded in the evidence artifact.
11. The upstream diff identity for the probe run is recorded in the evidence artifact.
12. A single atomic evidence artifact is produced for the run and stored durably.
13. The external adversarial review gate is satisfied only by the required reviewer workflow and evidence artifact.
14. No touched seam is left without a named canary.
15. No ambiguous upstream change remains unresolved.

If any item above fails, the probe must fail closed.

## Fail-Closed Rules

The workflow must fail closed in all of these cases:

1. Upstream NT revision cannot be resolved cleanly to an immutable SHA.
2. Lockfile refresh fails or is inconsistent with the resolved NT SHA.
3. Build or test suite fails.
4. A changed upstream NT path cannot be classified.
5. A required seam canary is missing.
6. A required canary is found but does not actually execute.
7. A touched seam does not have all required coverage classes satisfied.
8. An upstream change touches a seam but no matching proof is defined.
9. Bolt currently uses `nautilus_*` code with no seam owner.
10. A seam path-prefix is stale or non-matching.
11. Upstream diff classification is ambiguous.
12. The atomic evidence artifact is missing or incomplete.
13. The external adversarial review gate has not been satisfied mechanically.

Fail closed means:

- no automatic merge
- no non-draft PR
- no draft PR for failing or ambiguous runs
- clear report of why the probe stopped

## Upstream Diff Inference

The probe may inspect upstream commit messages and changed paths to determine what extra scrutiny is needed.

Allowed uses:

- requiring additional seam canaries
- tagging the probe result as ambiguous
- generating a compact reviewer summary
- deciding whether external adversarial review is especially important

Forbidden uses:

- skipping a registry-required seam
- declaring a bump safe when named seam proof is absent
- classifying an unmapped upstream path as safe
- overriding human review requirements

Inference is advisory only.

It must be treated as code under test:

- a historical replay set of known-dangerous upstream changes must be versioned alongside the registry
- the replay set must be owned by the same owner group as the seam registry
- the replay set must run on every inference-rule change and on a bounded periodic cadence
- if inference stops escalating known-dangerous patterns, the probe design has regressed
- a replay regression must block probe automation until fixed

## Draft PR Rules

If all required evidence passes, automation may open a draft PR that includes:

- the exact resolved NT SHA
- the new NT revision
- the previous NT revision
- lane name (`develop` or tagged release)
- seam registry version or hash
- safe-list version or hash
- a summary of touched seams
- a summary of which canaries passed and their coverage classes
- any advisory flags from upstream diff inference
- a link or attachment to the atomic evidence artifact
- the status of the external adversarial review gate

Draft PR creation must be strict:

- draft PRs must only be opened for fully passing probe runs
- failing or ambiguous runs must produce reports, not PRs
- there must be at most one open probe PR per lane
- a newer probe run for the same lane must supersede the older PR automatically
- a stale probe PR must close automatically after the configured staleness window

## Probe Artifact

Each successful probe run must emit one atomic machine-readable evidence artifact.

At minimum it must contain:

- lane name
- resolved NT SHA
- source ref name
- upstream diff hash or identity
- previous NT SHA
- registry version or hash
- safe-list version or hash
- Bolt-side `nautilus_*` usage inventory hash or manifest
- canary list
- canary coverage classes
- canary execution results
- toolchain and environment identifiers
- lockfile digest
- run timestamps
- supersedes or superseded-by metadata when applicable

The artifact must be stored durably outside the PR itself, with retention long enough to support postmortem and audit use.

No draft PR may be created without this artifact.

Partial runs, interrupted runs, or stale artifacts must never be treated as evidence.

## Probe Lifecycle

Probe runs must be reproducible and isolated.

Rules:

- every run starts from a fresh probe branch or worktree
- partial artifacts from a previous run must not be reused
- interrupted runs are invalid unless they produced a complete evidence artifact
- probe branches are ephemeral and owned by the workflow
- a probe PR is stale if its target SHA, registry version, or lane state is no longer current

## External Adversarial Review

External adversarial review is explicitly part of the decision flow, but it is out-of-band from the automated probe.

Responsibilities:

- the automated system prepares the evidence artifact and reviewer summary
- the human operator runs external model adversarial reviews
- the automated system does **not** perform those reviews itself

This review must be a real mechanical gate, not a note in the PR body.

Required design rule:

- every probe PR must carry a required status check named for external adversarial review
- that status must start failing or pending
- only designated reviewers may transition it to passing after review is complete
- designated reviewers must be defined concretely, not implicitly
- designated reviewers must be a named owner group or explicitly configured reviewer set
- the PR author or probe initiator must not be allowed to satisfy their own adversarial-review gate
- for solo-operator workflows, the passing transition must require an external-review artifact, not just a click

The external-review artifact must be concrete and durable:

- it must reference the exact NT SHA and probe artifact
- it must identify the seams reviewed
- it must record the review result in a durable location outside ephemeral chat history

Until that status is passing, the PR must not be mergeable.

## Run Reporting

Every probe run, whether pass or fail, must leave an auditable status record.

At minimum:

- each run updates a durable lane status surface
- missing probe activity beyond the configured alert threshold must raise an alert
- silent decay of the probe infrastructure is itself a failure mode
- the alerting surface must not rely on a single silent channel

## Decision Flow

Recommended high-level flow:

1. Resolve the lane target to an immutable full SHA.
2. Create a fresh isolated probe branch or worktree.
3. Update the pinned NT revision.
4. Update all pinned `nautilus-*` dependencies atomically to the same resolved NT SHA.
5. Refresh the lockfile.
6. Run the mechanical baseline in the required toolchain and environment.
7. Classify every changed upstream NT path using registry matching and safe-list rules.
8. Re-check registry completeness against current Bolt `nautilus_*` usage and seam path-prefix validity.
9. Determine required seams from the registry.
10. Use advisory inference only to add seams or escalate ambiguity.
11. Run all required seam canaries.
12. Produce and store the atomic evidence artifact.
13. If any requirement fails, emit a failure report and stop.
14. If all requirements pass, open or update the single draft PR for that lane.
15. Leave the external adversarial review status pending.
16. Hand off for human review and external adversarial review.

## Canary Requirements

Canaries must be selected to prove Bolt-owned semantics, not merely NT crate compilation.

Good canaries:

- bootstrap tests that exercise Bolt’s NT integration path
- reconciliation and execution tests at seam boundaries
- reference pipeline tests that prove data-flow assumptions
- persistence and lake conversion tests that prove downstream data contracts
- config render and load tests that prove materialized runtime compatibility

Bad canaries:

- generic compile-only checks that do not exercise seam behavior
- heuristics with no direct relation to the touched seam
- pure upstream metadata scans without Bolt execution evidence

Each canary entry must define:

- the seam it proves
- the coverage class it provides
- the path it executes
- the assertion surface it covers
- any fixture or recorded data dependencies it relies on

A seam is not considered proven by a compile-only check unless its declared coverage class is explicitly compile-time API compatibility.

Each seam entry must define the coverage classes it requires. A touched seam fails unless every required coverage class has at least one passing canary.

## Maintenance Model

The seam registry is expected to evolve. That is acceptable and required.

Whenever an NT bump exposes a previously-unmapped Bolt/NT overlap seam, the correct response is:

1. add or refine the seam entry
2. add a named canary if one does not exist
3. keep the workflow fail-closed until the seam is represented explicitly

The registry becoming more conservative over time is a feature, not a bug.

The maintenance model must also include:

- a designated owner for registry changes
- a bounded response expectation for registry-gap failures
- a bounded response expectation for ambiguity resolution
- periodic replay or audit of inference and registry assumptions

Ambiguity may not block indefinitely. If the same ambiguity blocks repeated probe runs beyond the configured threshold, the owner group must resolve it by one of:

- adding or fixing a seam mapping
- adding a justified safe-list entry
- recording an explicit operator risk acceptance outside the automated landing path

## Risks

### False Positive Risk

The probe will sometimes block safe bumps because the registry is conservative or the inference layer is suspicious.

This is acceptable. The user requirement is to avoid auto-landing bad bumps even at the cost of missing safe ones.

### Registry Drift Risk

If the seam registry is not maintained, it will stop reflecting real Bolt/NT overlap.

This is why the workflow must fail closed on unmapped ambiguity and unmapped Bolt-side NT usage rather than silently trusting old registry entries.

### Canary Coverage Risk

A named canary may pass without actually proving the seam strongly enough.

That is why canaries should be treated as explicit contracts with declared coverage classes and reviewed as first-class design artifacts, not incidental tests.

### Residual Runtime Risk

Some regressions will only manifest under real network conditions, real venue behavior, or long-lived runtime conditions.

This design reduces that risk but does not eliminate it.

## Success Criteria

This design is successful when:

1. NT drift is probed regularly in both `develop` and tagged-release lanes.
2. No bump can auto-land.
3. Draft PRs open only when Bolt-owned seam evidence is complete and recorded in an atomic evidence artifact.
4. Ambiguous or unmapped changes fail closed rather than slipping through.
5. Human review and external adversarial review are enforced mechanically before merge.
