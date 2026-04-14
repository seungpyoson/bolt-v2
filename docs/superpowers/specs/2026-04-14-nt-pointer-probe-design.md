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

The seam registry is the true gate. Advisory inference can only make the system stricter. It must never reduce required evidence.

This hierarchy is required because commit messages and file paths are too weak to detect all Bolt/NT semantic overlap. A false negative is the exact failure we are trying to avoid.

## Probe Lanes

The system should maintain two separate NT probe lanes.

### Lane 1: `develop`

Purpose:

- detect Bolt/NT incompatibility early
- keep upgrade deltas small
- surface draft PRs quickly when evidence is strong enough

Behavior:

- scheduled probe against upstream NT `develop`
- if all required evidence passes, automation may open a draft PR
- if evidence is incomplete or ambiguous, the probe reports failure and opens no PR

### Lane 2: tagged releases

Purpose:

- provide a calmer upstream target
- give the team a more conservative lane for merge candidates

Behavior:

- scheduled or manually-triggered probe against the newest NT tagged release
- same gating model as `develop`
- also draft-PR only, never auto-merge

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
- upstream NT paths or concepts likely to touch it
- Bolt-owned canary tests or probes required for proof
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

6. **Build and packaging surface**
   - Risk: lockfile churn, transitive crate conflicts, artifact generation drift
   - Likely evidence: build verification, CLI help and render-live-config smoke path, full CI build

### Initial Registry Clarification

The prior open concern around NT issue `#3806` (“Polymarket data client auto-subscribe vs. strategy-driven subscriptions”) should **not** remain an unresolved seam by itself.

Current evidence in Bolt suggests:

- Bolt builds explicit Polymarket data-client filters in [`src/clients/polymarket.rs`](/Users/spson/Projects/Claude/bolt-v2/src/clients/polymarket.rs:142)
- Bolt-owned actors drive subscriptions explicitly, for example in [`src/platform/reference_actor.rs`](/Users/spson/Projects/Claude/bolt-v2/src/platform/reference_actor.rs:237)

Therefore this concern should be covered under the broader “subscription and custom-data semantics” seam rather than treated as a separate unresolved blocker.

## Evidence Contract

A probe is only allowed to open a draft PR if **all** of the following are true:

1. The NT pointer is updated in an isolated probe branch.
2. The lockfile refresh succeeds.
3. The full repo CI-equivalent suite passes.
4. Every seam whose proof is required by the registry has passing evidence.
5. No touched seam is left without a named canary.
6. No ambiguous upstream change remains unclassified without escalation.

If any item above fails, the probe must fail closed.

## Fail-Closed Rules

The workflow must fail closed in all of these cases:

1. Upstream NT revision cannot be resolved cleanly.
2. Lockfile refresh fails.
3. Build or test suite fails.
4. A required seam canary is missing.
5. An upstream change appears to touch a sensitive seam but no matching proof is defined.
6. Upstream diff inference is ambiguous.
7. The seam registry has no entry for a newly-touched semantic overlap area.
8. External adversarial review is required by policy but has not yet happened.

Fail closed means:

- no automatic merge
- no non-draft PR
- optionally no draft PR at all if the ambiguity is severe enough
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
- overriding human review requirements

Inference is advisory only.

## Draft PR Rules

If all required evidence passes, automation may open a draft PR that includes:

- the new NT revision
- the previous NT revision
- lane name (`develop` or tagged release)
- a summary of touched seams
- a summary of which canaries passed
- any advisory flags from upstream diff inference
- an explicit note that external adversarial review is still required before implementation approval or merge

If evidence is incomplete, automation should prefer a machine-readable report over a PR. It should not create noisy draft PRs for obviously failing or ambiguous bumps unless the team explicitly chooses that behavior later.

## External Adversarial Review

External adversarial review is explicitly part of the decision flow, but it is out-of-band from the automated probe.

Responsibilities:

- the automated system prepares the design summary and evidence
- the human operator runs external model adversarial reviews
- the automated system does **not** perform those reviews itself

This design therefore requires a handoff point where the probe output is stable enough for external review, but before any merge decision.

## Decision Flow

Recommended high-level flow:

1. Resolve upstream NT target for the lane.
2. Create isolated probe branch/worktree.
3. Update pinned NT revision.
4. Refresh lockfile.
5. Run compile/build/test baseline.
6. Determine required seams from the authoritative registry.
7. Use upstream diff inference to add extra scrutiny if needed.
8. Run required seam canaries.
9. If any requirement is missing or ambiguous, fail closed.
10. If all evidence passes, open a draft PR with evidence summary.
11. Hand off for human review and external adversarial review.

## Canary Philosophy

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

## Maintenance Model

The seam registry is expected to evolve. That is acceptable and required.

Whenever an NT bump exposes a previously-unmapped Bolt/NT overlap seam, the correct response is:

1. add or refine the seam entry
2. add a named canary if one does not exist
3. keep the workflow fail-closed until the seam is represented explicitly

The registry becoming more conservative over time is a feature, not a bug.

## Risks

### False Positive Risk

The probe will sometimes block safe bumps because the registry is conservative or the inference layer is suspicious.

This is acceptable. The user requirement is to avoid auto-landing bad bumps even at the cost of missing safe ones.

### Registry Drift Risk

If the seam registry is not maintained, it will stop reflecting real Bolt/NT overlap.

This is why the workflow must fail closed on unmapped ambiguity rather than silently trusting old registry entries.

### Canary Coverage Risk

A named canary may pass without actually proving the seam strongly enough.

That is why canaries should be treated as explicit contracts and reviewed as first-class design artifacts, not incidental tests.

## Success Criteria

This design is successful when:

1. NT drift is probed regularly in both `develop` and tagged-release lanes.
2. No bump can auto-land.
3. Draft PRs open only when Bolt-owned seam evidence is complete.
4. Ambiguous bumps fail closed rather than slipping through.
5. Human review and external adversarial review remain explicit mandatory steps before merge.

