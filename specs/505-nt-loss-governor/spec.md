# Feature Specification: NT-First Loss Governor

**Feature Branch**: `codex/nt-loss-governor-circuit-breaker`
**Created**: 2026-06-01
**Status**: Draft
**Input**: Prompt `/Users/spson/Downloads/prompts/bolt-v2-circuit-breaker-goal.md`; GitHub issue #505.

**PR #507 Scope Note**: PR #507 implemented the pure loss-governor and positional-sizing core, plus the configured loss-governor gate in shared submit admission. It wires `[risk.loss_governor]` into `bolt_v3_live_node`, subscribes a configured NT portfolio/position/account runtime feed that publishes loss snapshots to submit admission, rejects entry/replace risk before NT submit on missing/stale/breached loss facts, leaves risk-reducing exits eligible under existing caps, records schema-v6 halt evidence, applies configured loss-halt actions through NT `RiskEngine::set_trading_state`, and exposes a live runtime manual-recovery method that can return NT risk state to `Active` only with fresh accepted loss evidence and bounded operator evidence. Active market exit is not part of this slice; any future active market-exit path must call NautilusTrader's owned `Trader::market_exit_strategy` primitive directly from a real live boundary. NT `Halted`/`Reducing` by itself does not prove the account is flat. The external operator clear-to-Active command surface remains later work.

## User Scenarios & Testing

### User Story 1 - Fresh Loss Snapshot Admission (Priority: P1)

As the operator, I need a shared Bolt loss governor to reject new risk when an NT-derived loss snapshot is stale, missing, or breaches per-trade or daily configured limits.

**Why this priority**: Fresh NT-derived loss/equity facts are the minimum safe input before any admission policy can claim loss-based protection. Without this, Bolt can keep admitting new risk after losses exceed configured policy.

**Independent Test**: Focused Rust tests instantiate the governor with configured limits and synthetic NT-derived snapshot facts, then verify admission rejects stale snapshots, per-trade loss breaches, and daily loss breaches without touching submit/live integration.

**Acceptance Scenarios**:

1. **Given** a fresh loss snapshot below every configured limit, **When** admission is evaluated, **Then** the decision accepts and records evidence freshness.
2. **Given** a fresh snapshot whose per-trade loss breaches the configured limit, **When** admission is evaluated, **Then** the decision rejects with halt reason `per_trade_loss_limit`.
3. **Given** a fresh snapshot whose daily loss breaches the configured limit, **When** admission is evaluated, **Then** the decision rejects with halt reason `daily_loss_limit`.
4. **Given** a stale, missing, or unattributed NT-derived snapshot, **When** admission is evaluated, **Then** the decision fails closed with halt reason `stale_loss_snapshot`.

### User Story 2 - Rolling Loss And Drawdown Admission (Priority: P2)

As the operator, I need the same governor to reject new risk when rolling-window loss or max drawdown breaches configured limits, while keeping all accounting facts sourced from NT-derived snapshots.

**Why this priority**: Daily loss alone cannot cover intra-session drawdown or non-calendar windows. This slice adds policy math without creating a second account truth.

**Independent Test**: Focused Rust tests provide configured rolling and drawdown limits with synthetic NT-derived facts, then verify the governor rejects only by the corresponding evidence reason.

**Acceptance Scenarios**:

1. **Given** a fresh snapshot whose rolling-window loss breaches the configured limit, **When** admission is evaluated, **Then** the decision rejects with halt reason `rolling_loss_limit`.
2. **Given** a fresh snapshot whose current equity is below peak equity by more than the configured max drawdown, **When** admission is evaluated, **Then** the decision rejects with halt reason `max_drawdown_limit`.
3. **Given** multiple configured limits breached, **When** admission is evaluated, **Then** the decision exposes deterministic halt evidence without depending on strategy-local logic.

### User Story 3 - Configured Submit/Live Integration (Priority: P3)

As the operator, I need configured loss-governor policy to reach the live submit-admission boundary so new entry/replace risk fails closed before NT submit when the NT-derived loss facts are missing, stale, or breached.

**Why this priority**: A pure evaluator is not production protection. The configured policy must be part of the shared submit-admission state that every strategy submit path uses.

**Independent Test**: Submit-admission tests prove missing snapshots fail closed before NT submit, breached snapshots halt entry risk, fresh below-limit snapshots admit, risk-reducing exits remain possible within the existing operator count cap, and live-node construction passes the configured policy into submit admission.

**Acceptance Scenarios**:

1. **Given** configured loss-governor policy and no fresh loss snapshot, **When** entry admission is evaluated, **Then** the decision rejects before NT submit with `stale_loss_snapshot`.
2. **Given** configured loss-governor policy and breached NT-derived facts, **When** entry admission is evaluated, **Then** the decision rejects before NT submit with deterministic loss halt reasons.
3. **Given** configured loss-governor policy and breached NT-derived facts, **When** a risk-reducing exit is evaluated, **Then** it remains eligible under the existing operator live-order count cap.
4. **Given** a live-node build from loaded TOML, **When** strategies are registered, **Then** the shared submit-admission state has the configured loss-governor policy.

## Edge Cases

- Snapshot exists but freshness timestamp is older than policy freshness bound.
- Snapshot exists but source attribution is missing.
- Snapshot has no configured currency facts for a configured loss policy.
- Negative PnL values are represented as losses; non-loss values must not trip loss thresholds.
- Missing enabled policy threshold must fail config validation instead of creating a hardcoded default.
- Fresh NT portfolio heartbeats must refresh aggregate loss/equity facts and evict expired rolling-window samples without letting historical peak equity make the snapshot stale.
- Multiple breach reasons must be reported deterministically.
- Risk-reducing exits must not be blocked by loss halt policy; existing operator count and lifecycle caps still apply.

## Requirements

### Functional Requirements

- **FR-001**: System MUST consume NT-derived loss/equity facts only; it MUST NOT compute independent account truth from venue fills or balances.
- **FR-002**: System MUST expose configured policy inputs for per-trade loss, daily loss, rolling-window loss, max drawdown, and snapshot freshness without hardcoded runtime defaults.
- **FR-003**: System MUST reject admission with reason `per_trade_loss_limit` when per-trade loss breaches its configured limit.
- **FR-004**: System MUST reject admission with reason `daily_loss_limit` when daily loss breaches its configured limit.
- **FR-005**: System MUST reject admission with reason `rolling_loss_limit` when rolling-window loss breaches its configured limit.
- **FR-006**: System MUST reject admission with reason `max_drawdown_limit` when drawdown from peak equity breaches its configured limit.
- **FR-007**: System MUST fail closed with reason `stale_loss_snapshot` when the loss snapshot is stale, missing, or unattributed.
- **FR-008**: System MUST bind configured loss-governor policy into the shared submit-admission state used by live strategies.
- **FR-009**: System MUST reject entry and replace submits before NT submit when configured loss-governor policy rejects, while leaving risk-reducing exits governed by existing lifecycle and count caps.
- **FR-010**: System MUST document NT support and NT gaps with exact pinned-source paths and line ranges.
- **FR-011**: System MUST use TDD vertical slices before production behavior changes.
- **FR-012**: System MUST NOT implement Bolt-built cancel, flatten, or bespoke venue side effects in this slice.
- **FR-013**: System MUST require every `[risk.loss_governor]` threshold when the governor is enabled.
- **FR-014**: System MUST evaluate loss snapshots against a conservative NT-derived observation timestamp for the facts in the snapshot and MUST evict expired rolling-window samples on fresh portfolio heartbeats.
- **FR-015**: System MUST validate explicit loss-governor trading-state and recovery-mode actions when the governor is enabled.
- **FR-016**: System MUST NOT add Bolt-owned active market-exit policy, config, or latch scaffolding in this slice.
- **FR-017**: System MUST use NautilusTrader's owned `Trader::market_exit_strategy` primitive directly if a later slice enables active market exit.

### Key Entities

- **LossGovernorPolicy**: Config-derived policy limits and freshness requirements.
- **LossSnapshot**: Fresh NT-derived PnL/equity facts plus source attribution and timestamp.
- **LossAdmissionDecision**: Accept/reject decision with deterministic halt evidence.
- **LossHaltReason**: Public reason enum for per-trade, daily, rolling, max-drawdown, and stale snapshot failures.
- **LossGovernorRuntimeFeed**: Live in-process feed that derives governor snapshots from NT `PortfolioSnapshot`, `AccountState`, and `PositionEvent` messages for the configured account.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Per-trade, daily, rolling-window, stale-snapshot, and max-drawdown regressions fail before implementation and pass after implementation.
- **SC-002**: `cargo test --locked --lib bolt_v3_loss_governor` passes.
- **SC-003**: `cargo test --locked --lib` passes.
- **SC-004**: `cargo fmt --check` and `git diff --check` pass.
- **SC-005**: Submit/live integration tests pass for `bolt_v3_submit_admission`.
- **SC-006**: `cargo test --locked --test config_parsing` and `cargo test --locked --test bolt_v3_decision_evidence` pass.
- **SC-007**: Final report separates submit-admission protection, NT trading-state protection, and the live manual-recovery method from flat-position proof, deferred active market-exit execution, and the external operator clear-to-Active command surface.

## Assumptions

- Current pinned NautilusTrader revision is `6e059dcbb59ac1e582132fc431a581936c216c3c`.
- Issue #505 is the tracking issue for this slice.
- PR #507 wires configured submit-admission loss protection, the live NT runtime feed that refreshes snapshots, explicit NT `RiskEngine::set_trading_state` side effects for configured loss halts, and the live runtime manual-recovery method. Active market-exit execution and the external operator clear-to-Active command surface remain later work.
