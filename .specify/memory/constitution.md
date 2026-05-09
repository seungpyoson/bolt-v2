<!--
Sync Impact Report
Version change: unratified template -> 1.0.0
Modified principles:
- Template Principle 1 -> Evidence-First Architecture
- Template Principle 2 -> Generic Contract Boundaries
- Template Principle 3 -> Single Source Runtime Configuration
- Template Principle 4 -> NT-First Pure Rust Runtime
- Template Principle 5 -> Empirical Readiness And Review Gates
Added sections:
- Scope And Source Of Truth
- Bolt-v3 Nucleus Admission Rules
- Review And Delivery Workflow
Removed sections:
- Placeholder Section 2
- Placeholder Section 3
Templates requiring updates:
- .specify/templates/plan-template.md: checked, no update required; Constitution Check section remains the gate.
- .specify/templates/spec-template.md: checked, no update required; requirements and success criteria sections are sufficient.
- .specify/templates/tasks-template.md: checked, no update required; generated tasks must replace sample content.
Deferred items: none.
-->

# Bolt-v2 Constitution

## Core Principles

### I. Evidence-First Architecture

Architecture claims MUST be backed by current evidence from source code, issue
history, PR patches, tests, upstream docs, or live probes as appropriate. Plans
MUST name the evidence used and MUST distinguish current `main` from stale
branches, old worktrees, forensic notes, or superseded drafts. No implementation
may proceed from momentum, fixture convenience, or an unverified prior claim.

### II. Generic Contract Boundaries

Generic core code MUST NOT name concrete providers, venues, market families,
strategies, market IDs, symbols, quantities, timeouts, feeds, chains, or runtime
deployment values. Concrete names such as Polymarket, Binance, Kalshi,
Hyperliquid, Chainlink, BTC, updown, or `binary_oracle_edge_taker` may appear
only in provider-owned bindings, family-owned bindings, explicit fixtures,
catalog/config data, tests that assert fencing, or documentation that records
evidence. New sessions MUST NOT turn fixture names into architecture.

### III. Single Source Runtime Configuration

Every runtime value MUST come from TOML configuration or a typed catalog reached
from configuration. The codebase MUST have one config shape, one runtime mapping
path, one deployment path, and one secret source. Values that change together
MUST live together so a wallet, credential set, venue, strategy, market family,
or feed registration can be changed in one config section. No env fallback,
CLI fallback, local file fallback, 1Password fallback, AWS CLI subprocess, or
second secret backend is allowed.

### IV. NT-First Pure Rust Runtime

The production runtime MUST be a standalone Rust binary using NautilusTrader's
Rust API directly. Python live-engine layers, PyO3, maturin, pip-based runtime
paths, duplicate backtest/live engines, and pass-through wrappers that recreate
NT behavior are not allowed. The design MUST define a thin boundary around NT
contracts, including BacktestEngine/live parity where strategy behavior depends
on data or decisions.

### V. Empirical Readiness And Review Gates

Readiness is facts-only until venue, data, risk, reconciliation, restart,
crash, latency, and recovery behavior are proven. Live orders require explicit
operator approval and venue-specific proof. Required verification MUST be run
before completion claims. A tracked issue is not a waiver unless the user
explicitly says so; every substantive review issue remains a finding until
fixed or waived.

## Scope And Source Of Truth

`main` is authoritative after merge. Old feature branches and worktrees are
forensic only. Stale branches may provide evidence, but accepted scope must be
ported to a fresh branch from `main` before it becomes implementation proof.

Each branch, PR, spec, or issue slice MUST declare one scope. A PR may close a
broader issue only when its diff satisfies that broader issue. Hidden adjacent
work and missing claimed scope are review findings.

The legacy Bolt v1 repository at `~/Projects/Claude/bolt/` MUST NOT be read,
imported, or used as a dependency. NautilusTrader source comes from the git
cache under `~/.cargo/git/checkouts/nautilus_trader-*` or from GitHub.

## Bolt-v3 Nucleus Admission Rules

The first forward Bolt-v3 milestone is a nucleus admission gate, not legacy
strategy migration and not a provider-specific live slice.

The nucleus MUST prove:

- config-owned runtime values;
- provider, market-family, and strategy-archetype contracts;
- conformance harnesses for those contracts;
- decision-event contract suitable for NT custom data;
- BacktestEngine/live parity boundary;
- zero concrete venue behavior outside fake/test bindings and fenced fixtures;
- provider leak and runtime literal guards that cannot be bypassed by narrow
  allowlists.

The nucleus MUST block generic code from carrying concrete updown plan or clock
types. The nucleus MUST treat the existing Polymarket/Binance/updown/BTC/
Chainlink fixtures as evidence and tests, not as the first architecture slice.

## Review And Delivery Workflow

Substantial work SHOULD start with spec-kit artifacts unless the user narrows
the task to an exact command, a small fix, or read-only research. The sequence is
constitution, specify, clarify if needed, plan, tasks, analyze, implement.

Plans MUST resolve all clarification markers before implementation. Tasks MUST
be independently testable and grouped by user-visible or operator-visible
value. Tests or verifiers MUST be added before implementation when the change is
a feature, bug fix, contract guard, or regression defense.

Before pushing a branch where CI matters, use the installed no-mistakes gate
unless the user explicitly waives it. Do not request external red-team review
while the branch has uncommitted changes, unpushed commits, unresolved findings,
unanswered review comments, or failing checks.

## Governance

This constitution is the controlling policy for spec-kit artifacts in this
repository. It incorporates the repo-level `AGENTS.md` rules and the current
Bolt-v3 recovery direction. If this file conflicts with a deeper future
`AGENTS.md`, the deeper file controls for that subtree; otherwise this
constitution controls planning and task generation.

Amendments require an explicit user request or a current evidence finding that
the constitution no longer matches the repo rules. Version changes follow
semantic versioning:

- MAJOR: removes or redefines a core principle.
- MINOR: adds or materially expands a principle or mandatory gate.
- PATCH: clarifies wording without changing obligations.

All specs, plans, tasks, PRs, and reviews MUST check compliance with this
constitution. Violations must be either fixed before implementation or recorded
as explicit complexity with user-approved rationale.

**Version**: 1.0.0 | **Ratified**: 2026-05-09 | **Last Amended**: 2026-05-09
