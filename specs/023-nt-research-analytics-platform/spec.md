# Feature Specification: NT-First Research Analytics Platform

**Feature Branch**: `023-nt-research-analytics-platform`
**Created**: 2026-05-20
**Status**: Draft
**Input**: User requested NT-first planning for flexible backtesting, comprehensive data capture, research analytics, current-trade dashboards, and issue-ready task slices across Polymarket, Hyperliquid HIP-4, Kalshi, Hyperliquid perps, Bybit, and related providers.

## Evidence Authority

`evidence.md` controls this specification. Each requirement must trace to a
`SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or `DECISION_NEEDED` row before it can
be turned into implementation tasks or issue payloads.

## User Scenarios & Testing

### User Story 1 - Prove NT Capabilities Before Building (Priority: P1)

As the operator, I need a current evidence pack showing which required backtesting, catalog, adapter, reporting, and dashboard surfaces NT already provides, so Bolt does not rebuild NT.

**Why this priority**: Every later slice depends on not duplicating NT order lifecycle, backtest, catalog, portfolio, or adapter behavior.

**Independent Test**: A reviewer can inspect the evidence pack and see exact NT release, commit, file/doc links, capability matrix, gaps, and go/no-go result for each venue and surface.

**Acceptance Scenarios**:

1. **Given** selected NT pointer evidence, **When** the HIP-4 lifecycle matrix is reviewed, **Then** every required operation is marked as NT standard path, NT client API, Bolt thin glue, or out of scope.
2. **Given** selected NT pointer evidence, **When** the dashboard source contract is reviewed, **Then** live PnL, positions, orders, fills, and account state are sourced from NT reports/events/snapshots or marked exploratory.
3. **Given** selected NT pointer evidence, **When** backtesting architecture is reviewed, **Then** Bolt uses NT `BacktestNode`/`ParquetDataCatalog` rather than a custom simulator.

---

### User Story 2 - Choose Data Providers With Cost And Fidelity Gates (Priority: P1)

As the operator, I need data-provider choices backed by cost and fidelity evidence, so research can use comprehensive data without silently exceeding the monthly cap or overstating backtest quality.

**Why this priority**: Provider choice controls whether selected perpetual-futures venues, Polymarket, HIP-4, and Kalshi backtests are execution-quality, trade/bar-only, signal-only, or forward-capture pending.

**Independent Test**: A reviewer can inspect one cost table and one fidelity matrix per provider mode and see the selected mode, rejected modes, monthly run-rate estimate, and affected venue coverage.

**Acceptance Scenarios**:

1. **Given** Tardis, Telonex, Goldsky, official archives, and official API capture options, **When** provider selection is reviewed, **Then** each option has cost, venue coverage, data classes, licensing, freshness, and NT catalog fit recorded.
2. **Given** a monthly cap, **When** the selected provider mode is reviewed, **Then** provider plus AWS plus dashboard run-rate remains under the cap or records explicit user waiver.
3. **Given** a venue with incomplete historical depth, **When** backtest readiness is reviewed, **Then** the feature labels it as L2 replay, trade/bar replay, signal-only, or forward-capture pending.

---

### User Story 3 - Run Research Backtests Through NT (Priority: P2)

As a researcher, I need manifest-driven NT backtest runs over cataloged data, so strategies can be explored flexibly while preserving lineage and live-path compatibility.

**Why this priority**: Research and alpha discovery require repeatable runs over multiple venues and data classes.

**Independent Test**: A reviewer can run or inspect a sample manifest and see exact NT config mapping, input catalog lineage, data fidelity class, output reports, and run hash.

**Acceptance Scenarios**:

1. **Given** cataloged data and a run manifest, **When** a backtest run is configured, **Then** every manifest field maps to an NT config field or Bolt orchestration metadata.
2. **Given** a backtest result, **When** analytics consumes it, **Then** the result references catalog/source hashes, NT pointer, strategy config hash, and fill model.
3. **Given** Python/Jupyter research output, **When** it is reviewed for production use, **Then** it is either research-only or graduated into production-compatible typed config and runtime contract.

---

### User Story 4 - See Current Trades And Outlook In A Read-Only Dashboard (Priority: P2)

As the operator, I need a visually clear dashboard for current trades, outlook, cumulative/historical PnL, exposure, data health, and strategy state, so I can understand what is happening without opening raw logs.

**Why this priority**: Live/readiness operations need fast human situational awareness, but dashboard must not become a trading control plane.

**Independent Test**: A reviewer can inspect dashboard source contract and verify that displayed trading truth comes from NT-derived read models with freshness/staleness indicators and no mutation actions.

**Acceptance Scenarios**:

1. **Given** NT reports/events/snapshots, **When** the live ops dashboard updates, **Then** displayed PnL, positions, orders, fills, exposure, and account state come from NT-derived sources.
2. **Given** stale or delayed data, **When** dashboard is viewed, **Then** freshness state is visible and stale data is not presented as current.
3. **Given** dashboard UI, **When** controls are inspected, **Then** no order submit, cancel, transfer, or credential mutation action exists.

---

### User Story 5 - Convert Plan Into Issue-Ready Slices (Priority: P3)

As the maintainer, I need dependency-ordered tasks and issue payloads, so follow-on work can proceed in small reviewable slices without duplicating existing issues.

**Why this priority**: Work must be split across cost gates, NT proofs, provider prototypes, backtest runner, analytics read model, and dashboard.

**Independent Test**: A reviewer can inspect tasks and issue payloads and see one declared slice per issue, accepted scope, residual scope, dependencies, and links to existing related issues.

**Acceptance Scenarios**:

1. **Given** existing issues #19, #20, #21, #22, #23, #24, #34, #36, #39, #75, #77, #88, #112, #115, #127, #148, #158, #176, #236, #254, #369, #385, #407, and #409, **When** new issue payloads are reviewed, **Then** each payload states whether it updates, depends on, or avoids duplication with existing issues.
2. **Given** issue payloads, **When** each is reviewed, **Then** it has a concrete problem, solution approach, why it exists, acceptance evidence, and reviewer risk notes.

### Edge Cases

- NT release supports a venue but current Bolt pin cannot upgrade cleanly.
- Provider cost fits data subscription but exceeds cap after AWS, dashboard, transfer, and query costs.
- Provider has trades/bars but no historical L2 book data.
- Historical data exists but does not include HIP-4 outcomes or Kalshi order-book snapshots.
- Dashboard ingest lags or drops data while live runtime continues.
- Research notebook contains strategy logic that could become a shadow runtime.
- NT report surface and analytics read model disagree.
- Existing issue already covers part of a proposed slice.

## Requirements

### Functional Requirements

- **FR-001**: System MUST use NT `BacktestNode`/`BacktestEngine` capabilities before any Bolt-owned backtest behavior is proposed.
- **FR-002**: System MUST use NT `ParquetDataCatalog` and NT core data types where available before defining custom data types.
- **FR-003**: System MUST treat Bolt run manifests as thin NT config mappings plus orchestration metadata, not a competing domain language.
- **FR-004**: System MUST include a cost gate covering data providers, AWS storage, compute, dashboard/BI, logs/metrics, query cost, and transfer.
- **FR-005**: System MUST include a fidelity gate for each venue/data source: L2 replay, trade/bar replay, signal-only, or forward-capture pending.
- **FR-006**: System MUST preserve Kalshi adapter support as `USER_ASSUMPTION` until selected-pointer proof verifies its adapter, data, order, report, and backtest surfaces.
- **FR-007**: System MUST use upstream NT Hyperliquid HIP-4 support before adding any Bolt HIP-4 behavior.
- **FR-008**: System MUST separately prove HIP-4 live support and HIP-4 historical backtest data availability.
- **FR-009**: System MUST preserve one source-of-truth chain: raw evidence -> deterministic NT catalog projection -> NT reports/results -> analytics/dashboard views.
- **FR-010**: System MUST prevent analytics/read models from becoming independent PnL, position, or account truth.
- **FR-011**: System MUST keep dashboards read-only with no trading, transfer, credential, or runtime mutation actions.
- **FR-012**: System MUST expose dashboard freshness/staleness for live PnL, positions, orders, fills, and data feeds.
- **FR-013**: System MUST keep Python/Jupyter research outside the production trading runtime unless an explicit production-compatible NT path is approved.
- **FR-014**: System MUST identify existing related GitHub issues before proposing new issue payloads.
- **FR-015**: System MUST use third-party providers or products when they meet cost, fidelity, security, and NT-fit gates better than building local infrastructure.
- **FR-016**: System MUST keep all runtime values, venue/product/provider identifiers, and adapter binding choices in TOML or NT-native config/registry selected by TOML; all live credentials MUST remain in AWS SSM.
- **FR-017**: System MUST classify every planning claim as source-proven, user assumption, gap, or decision-needed before using it as task scope.

### Key Entities

- **NT Capability Matrix**: Evidence table for NT version, venue, data class, adapter, backtest, catalog, report, and gap status.
- **Provider Cost Model**: Monthly run-rate table for subscriptions, AWS, dashboard/BI, logging, query, transfer, and reserve.
- **Data Fidelity Class**: Label describing allowable backtest claims for a venue/data source.
- **Raw Evidence Record**: Immutable provider payload plus capture metadata and content hash.
- **Catalog Projection**: NT catalog data derived from raw evidence or provider replay with transform/version lineage.
- **Run Manifest**: Thin, validated map to NT backtest/live config plus Bolt orchestration metadata.
- **Research Result**: Backtest/report/analysis output linked to NT pointer, run hash, catalog lineage, strategy config, and fidelity class.
- **Dashboard Read Model**: Read-only view derived from NT reports/events/snapshots and analytics outputs.
- **Issue Slice**: One dependency-bounded unit with problem, solution, acceptance evidence, risk, and existing-issue relationship.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Every planned venue has an NT capability row and provider/fidelity row before implementation tasks are issued.
- **SC-002**: Every selected provider mode has a monthly cost estimate under the approved cap or an explicit waiver.
- **SC-003**: Every backtest task names its NT entry point, data class, fill model, catalog path contract, and fidelity label.
- **SC-004**: Every dashboard task names its NT-derived source for PnL, positions, orders, fills, and freshness.
- **SC-005**: Every new issue payload links or references existing related issues and names residual scope.
- **SC-006**: Every issue payload traces to `evidence.md` and has no unresolved `GAP` blocking its claimed scope.
- **SC-007**: Dashboard MVP work starts only after existing BI/observability products have been evaluated against source contract, security, query-backend, UX, and all-in cost.

## Assumptions

- Kalshi adapter support is user-supplied and should be treated as usable unless capability proof contradicts it.
- Cost is acceptable up to the approved monthly cap; spending for reliable data is preferred over rebuilding commodity provider infrastructure.
- Research notebooks are useful for alpha discovery but are not production live trading runtime.
- Trade readiness work remains separate from this planning spec unless an explicit dependency is named.
- Existing dirty/untracked worktree files are user-owned and are not part of this spec unless explicitly referenced.
