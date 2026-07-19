<!--
Sync Impact Report
Version change: 2.1.1 -> 2.2.0
Modified principles: V. Evidence Before Claims records the single fixed final-review evidence transaction
Additional Constraints: unchanged
Added sections: v2.2.0 migration note
Removed sections: none
Templates reviewed: .specify/templates/plan-template.md - no update needed;
.specify/templates/spec-template.md - no update needed;
.specify/templates/tasks-template.md - no update needed;
.specify/templates/constitution-template.md - no update needed;
.specify/templates/commands - absent in this repo
Runtime guidance updated: AGENTS.md establishes the fixed preflight, publication, and final-review path while preserving single-PR queue mechanics
Follow-up items: none
-->

# Bolt-v3 Constitution

## Core Principles

### I. NT-First Thin Layer

Bolt-v3 MUST remain a thin Rust layer over NautilusTrader. Bolt-v3 owns TOML schema parsing, SSM-only secret resolution, provider/market/strategy registration, strategy decision policy, pre-submit admission gates, and compact audit evidence for Bolt-derived decisions.

NautilusTrader owns runtime adapter behavior, protocols, market data, execution, order lifecycle, cache semantics, portfolio/account/order/fill state, reconciliation, and venue wire translation. Bolt-v3 MUST NOT rebuild those surfaces with local order lifecycle machinery, reconciliation machinery, mock venue worlds, or adapter simulators as proof of live readiness.

### II. Generic Core, Concrete Edges

Bolt-v3 core MUST be venue-agnostic, market-family-agnostic, and strategy-agnostic. Concrete provider keys, market-family keys, strategy archetypes, and NT adapter bindings live only in registry or binding modules selected by TOML configuration.

Adding a venue, market family, or strategy MUST NOT require changing core build, secret, admission, or runtime loop logic. If a concrete provider leaks into core, the slice fails the constitution gate.

### III. Single Path And Config-Controlled Runtime

There is one config format, one secret source, one production build path, one manifest-bound content-addressed immutable install path, and one live submit admission path through the exact installed binary's `ops launch`. Every runtime value comes from TOML configuration. Product/runtime credentials resolve only from AWS SSM through the Rust AWS SDK. Environment variable fallbacks, Python runtime layers, hardcoded IDs, hardcoded quantities, hardcoded timeouts, alternate installers, mutable-copy launch targets, install/launch compatibility adapters, and alternate submit paths are forbidden. `JULES_API_KEY` is allowed only as a GitHub Actions secret for repository code-maintenance advisory workflows; it is not a product/runtime/deploy/live/trading secret, not an alternate GitHub token, and must not be exposed to AWS, market data, order execution, runtime, deploy, or live jobs.

Changing a wallet, credential set, venue, target market, strategy, notional cap, timing bound, or approval token must require editing one coherent TOML section, not scattered code or multiple config locations.

### IV. Evidence-Driven Verification Gates

Implementation MUST be evidence-driven and risk-appropriate. TDD is permitted and
often useful, but it is not mandatory unless the user, active spec, or risk
analysis explicitly requires it. Every change MUST have current evidence before
readiness is claimed. Detailed agent workflow belongs in `AGENTS.md`.

The approved merge-governance state has zero required CI statuses. CI is visible evidence, not merge authority; native code-owner approval, stale-review dismissal, last-push approval, and human review-thread resolution remain mandatory. The repository accepts temporarily red or broken `main` as repository risk, never as deploy or trading permission. Repository admission is limited to identity, pull-request state and mergeability, required-reviewer approval, Mergify routing, and single-PR queue mechanics.

Live trading stays fail-closed. No live submit may occur unless production entrypoint, live canary gate, submit admission, mandatory decision evidence, no-submit readiness evidence, configured caps, and explicit operator approval all pass on the exact head being run. The exact installed executable must also validate its selected manifest and config bundle and complete its finite in-process pre-arm phase. Only complete success may construct the opaque, non-serializable, non-cloneable, one-use Rust `LiveReadinessPermit` consumed by the sole Start entrypoint. Installation, advisory CI, prior results, caches, tags, same-SHA evidence, and persisted receipts cannot authorize Start. Every start or restart requires a fresh permit.

### V. Evidence Before Claims

Claims about readiness require current evidence from exact files, exact commands,
exact SHAs, exact PR/check state, or live run artifacts. Passing tests, static
checks, reviews, or local mocks are not readiness evidence unless the checked
behavior covers the stated requirement.

External review is requested only after the branch is clean, pushed, and all local findings are resolved. The single fixed final-review workflow always executes the same root, Backtester, repository-static, and host workload. It invokes every configured reviewer against one captured head only after the complete workload passes. Its results are evidence for claims, but advisory automation cannot authorize or veto review or merge. no-mistakes may be used for task triage and branch gating, but its output is advisory until mapped to concrete repo evidence.

### VI. Minimal Slice Discipline

One branch or PR covers one named slice. Slices must be independently reviewable and must name residual scope. Prefer deletion, reuse of NT surfaces, and compact contracts over new frameworks. Do not expand verifier ecosystems for test-local literals, mock venue universes, or documentation stacks that do not reduce live-trading risk.

Backtesting and research analytics are valuable but outside the tiny-capital live-canary MVP unless they are required to prove the canary safety gate. They belong in a separate spec when the running production-shaped spine exists.

### VII. Research And Analytics Stay NT-First

Backtesting, research analytics, data analytics, and dashboards MUST use NautilusTrader vocabulary and surfaces before adding Bolt-owned machinery. NT owns backtest engines, catalog types, order/fill/account/portfolio state, reports, snapshots, and venue adapter semantics. Bolt may orchestrate TOML-driven runs, SSM-backed credentials, provider capture, deterministic catalog projection, lineage, read-only read models, and dashboards.

Research notebooks are allowed only as research surfaces. They MUST NOT become a production trading runtime, hidden submit path, or second strategy authority. Any productionized strategy behavior must graduate into the production runtime contract with typed config and verification.

Dashboards and analytics MUST be read-only. They MUST NOT submit orders, cancel orders, transfer funds, mutate credentials, or define independent PnL/position truth. Live PnL, positions, orders, fills, exposure, and account state must come from NT reports, events, snapshots, or explicitly marked exploratory derived data.

External data providers are allowed when they avoid rebuilding commodity data infrastructure. Provider choice MUST pass a cost and fidelity gate. Spending more to buy reliable data is acceptable, but total recurring provider plus AWS plus dashboard cost must remain under the approved monthly cap unless the user explicitly waives it.

## Additional Constraints

- Language/runtime: pure Rust binary using NautilusTrader Rust APIs directly.
- Secret source: AWS SSM through Rust AWS SDK only for product/runtime credentials. `JULES_API_KEY` is allowed only as a GitHub Actions secret for repository code-maintenance advisory workflows and must not reach AWS, market data, order execution, runtime, deploy, or live jobs.
- Runtime config: TOML only.
- Research/backtest config: TOML or NT-native run config only, with direct field mapping and lineage.
- Current repo source of truth: `main` after merge.
- Current live proof boundary: real SSM and real venue artifacts, exact installed bytes, and a fresh in-process readiness permit, not mock-only tests, advisory results, inherited evidence, or persisted authority.
- Old Bolt v1 repository is forbidden as a source.
- Raw secrets, private keys, and credential values must never be printed in docs, logs, test output, PRs, or chat.

## Development Workflow

1. Evidence: inspect current `main`, exact file paths, exact lines, exact command output.
2. Contract: update this constitution or feature contracts before runtime code when the boundary changes.
3. Plan: decompose into independently reviewable slices with a named verification approach.
4. Implementation: collect current evidence before claiming completion.
5. Review: no external review request until the local branch is clean, pushed, known findings are resolved, and the fixed final-review workflow can evaluate the exact head.
6. Merge: no merge without explicit user approval.

## Governance

This constitution is the SpecKit project-principles artifact. `AGENTS.md` is the
repo governance and agent workflow source. Any PR that violates a MUST rule in
this artifact requires redesign, not waiver-by-documentation. Amendments require
an explicit user-approved diff, a migration note for affected specs/plans, and a
version bump.

Migration note for v2.2.0: repository verification uses one workspace registry,
one non-compile preflight, exact-SHA publication, and one fixed final-review
transaction. Single-PR queue preflight remains the team-controlled Mergify
entrypoint and does not interpret advisory evidence as merge authority.

Migration note for v2.1.1: the combined #1398, #1399, and #1400 cutover removes
staged CI, Mergify-predicate, and queue-preflight merge authority. No active
feature spec or plan requires migration; this amendment aligns project principles
with the single approved cutover path.

Migration note for v2.1.0: the approved lean-CI architecture selected zero
required CI statuses, native human merge authority, one informational exact-
binary lane, and one manifest-bound immutable `ops launch` path whose fresh
in-process permit alone can cross Start. The historical trusted-App/precursor
control-plane design was superseded. That amendment alone changed no workflow,
ruleset, Mergify, deploy, launch, or trading state.

Migration note for v2.0.1: Jules advisory workflow planning may use `JULES_API_KEY`
only as a GitHub Actions code-maintenance automation token. This amendment does
not change the product/runtime secret source, live proof boundary, or submit
admission path.

Migration note for v2.0.0: affected planning artifacts must replace blanket TDD
language with evidence-driven verification. Active work under
`specs/026-nt-backed-iv-engine/` is updated by this amendment. Historical specs
remain archival unless reopened from current `main`. Operational agent guidance
for plugins and generated prompts lives in `AGENTS.md`.

Migration note for v1.1.0: affected planning artifacts are
`specs/023-nt-research-analytics-platform/spec.md`,
`specs/023-nt-research-analytics-platform/plan.md`,
`specs/023-nt-research-analytics-platform/archive/research.md`,
`specs/023-nt-research-analytics-platform/reference/evidence.md`,
`specs/023-nt-research-analytics-platform/reference/data-model.md`,
`specs/023-nt-research-analytics-platform/reference/contracts.md`,
and `specs/023-nt-research-analytics-platform/tasks.md`. Runtime code is not
changed by this amendment; implementation remains gated by feature-branch
SpecKit checks and exact-head verification.

**Version**: 2.2.0 | **Ratified**: 2026-05-12 | **Last Amended**: 2026-07-19
