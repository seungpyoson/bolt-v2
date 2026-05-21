<!--
Sync Impact Report
Version change: 1.0.0 -> 1.1.0
Modified principles: added VII. Research And Analytics Stay NT-First
Added sections: Research/backtest config constraint; v1.1.0 migration note
Removed sections: none
Templates reviewed: .specify/templates/plan-template.md - no update needed;
.specify/templates/spec-template.md - no update needed;
.specify/templates/tasks-template.md - no update needed;
.specify/templates/commands - absent in this repo
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

There is one config format, one secret source, one production build path, and one live submit admission path. Every runtime value comes from TOML configuration. Credentials resolve only from AWS SSM through the Rust AWS SDK. Environment variable fallbacks, Python runtime layers, hardcoded IDs, hardcoded quantities, hardcoded timeouts, and alternate submit paths are forbidden.

Changing a wallet, credential set, venue, target market, strategy, notional cap, timing bound, or approval token must require editing one coherent TOML section, not scattered code or multiple config locations.

### IV. Test-First Safety Gates

Implementation MUST be TDD. For every production behavior change: write the failing test, verify the expected failure, implement the smallest code change, verify green, then run the phase verification gate.

Live trading stays fail-closed. No live submit may occur unless production entrypoint, live canary gate, submit admission, mandatory decision evidence, no-submit readiness evidence, configured caps, and explicit operator approval all pass on the exact head being run.

### V. Evidence Before Claims

Claims about readiness require current evidence from exact files, exact commands, exact SHAs, exact PR/check state, or live run artifacts. Passing tests or local mocks are not live readiness unless the checked behavior covers the stated live requirement.

External review is requested only after the branch is clean, pushed, all local findings are resolved, and exact-head CI is green. no-mistakes may be used for task triage and branch gating, but its output is advisory until mapped to concrete repo evidence.

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
- Secret source: AWS SSM through Rust AWS SDK only.
- Runtime config: TOML only.
- Research/backtest config: TOML or NT-native run config only, with direct field mapping and lineage.
- Current repo source of truth: `main` after merge.
- Current live proof boundary: real SSM and real venue artifacts, not mock-only tests.
- Old Bolt v1 repository is forbidden as a source.
- Raw secrets, private keys, and credential values must never be printed in docs, logs, test output, PRs, or chat.

## Development Workflow

1. Evidence: inspect current `main`, exact file paths, exact lines, exact command output.
2. Contract: update this constitution or feature contracts before runtime code when the boundary changes.
3. Plan: decompose into independently reviewable slices with tests and verification commands.
4. TDD implementation: red, green, refactor, verification gate for each behavior slice.
5. Review: no external review request until local branch is clean, pushed, exact-head checks are green, and known findings are resolved.
6. Merge: no merge without explicit user approval.

## Governance

This constitution supersedes convenience, local habit, and stale branch artifacts. Any PR that violates a MUST rule requires redesign, not waiver-by-documentation. Amendments require an explicit user-approved diff, a migration note for affected specs/plans, and a version bump.

Migration note for v1.1.0: affected planning artifacts are
`specs/023-nt-research-analytics-platform/spec.md`,
`specs/023-nt-research-analytics-platform/plan.md`,
`specs/023-nt-research-analytics-platform/shared/research.md`,
`specs/023-nt-research-analytics-platform/shared/evidence.md`,
`specs/023-nt-research-analytics-platform/shared/contracts.md`,
and `specs/023-nt-research-analytics-platform/tasks.md`. Runtime code is not
changed by this amendment; implementation remains gated by feature-branch
SpecKit checks and exact-head verification.

**Version**: 1.1.0 | **Ratified**: 2026-05-12 | **Last Amended**: 2026-05-20
