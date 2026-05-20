# Feature Specification: NT-Matched Maker Order Scope

**Feature Branch**: `codex/maker-order-proof`  
**Created**: 2026-05-20  
**Status**: Draft  
**Input**: User request to enable maker orders only after evidence-based end-to-end investigation, matching NautilusTrader conventions and using Speckit task control.

## User Scenarios & Testing

### User Story 1 - Evidence-Gated Maker Scope (Priority: P1)

As an operator, I need the supported maker-order scope to be derived from the pinned NautilusTrader Polymarket adapter, not from local guesses.

**Why this priority**: Any implementation before this proof risks adding inverse work, bloated local behavior, or behavior NT does not support.

**Independent Test**: A reviewer can inspect `research.md` and line-referenced source evidence to see exactly what NT supports and what bolt-v3 must expose.

**Acceptance Scenarios**:

1. **Given** the pinned NautilusTrader rev in `Cargo.toml`, **When** the investigation is complete, **Then** the spec names supported maker combinations with NT source line evidence.
2. **Given** current bolt-v3 source, **When** config-to-submit path is mapped, **Then** every handoff from TOML to NT `submit_order` has file and line evidence.
3. **Given** branch commit `97cbf828423578e09a604bf31bdaa91ec3573df3`, **When** the process evidence is reviewed, **Then** it is treated as a candidate implementation rather than proof that review and TDD gates were followed before the commit.

### User Story 2 - Config-Driven Maker Orders (Priority: P2)

As a strategy author, I need to choose maker entry and maker exit behavior through TOML order parameters, preserving NT naming and the existing single runtime path.

**Why this priority**: Maker behavior must be enabled without hardcoded runtime values or parallel order paths.

**Independent Test**: A strategy TOML using supported maker combinations validates, maps into runtime config, builds NT orders with the expected TIF and `post_only`, and still submits through the existing NT submit path.

**Acceptance Scenarios**:

1. **Given** entry order config `order_type=limit`, `time_in_force=gtc`, `is_post_only=true`, **When** config validates and the strategy builds an order, **Then** the NT order has `TimeInForce::Gtc` and `is_post_only=true`.
2. **Given** exit order config `order_type=limit`, `time_in_force=gtc`, `is_post_only=true`, **When** config validates and the strategy builds an order, **Then** the NT order has `TimeInForce::Gtc` and `is_post_only=true`.
3. **Given** NT supports `Gtd` post-only limit orders, **When** bolt-v3 considers exposing GTD, **Then** implementation is blocked until an explicit TOML-owned expiry contract is approved.

### User Story 3 - Review-Approved Implementation (Priority: P3)

As a maintainer, I need internal and external review gates before implementation and after implementation, with no claims beyond evidence.

**Why this priority**: The user explicitly requires subagent-driven development, TDD, adversarial review, and external consensus before proceeding.

**Independent Test**: Review records show researcher, adversarial reviewer, implementer, and auditor stages, including Claude, Gemini, Kimi, DeepSeek, and GLM outputs where available.

**Acceptance Scenarios**:

1. **Given** investigation artifacts, **When** adversarial review runs, **Then** implementation does not proceed unless findings are resolved or explicitly waived.
2. **Given** implemented code, **When** audit review runs, **Then** the branch is not committed/pushed as complete until all required checks pass.

## Edge Cases

- Post-only with market order must fail before NT submit.
- Post-only limit order with `fok` or `ioc` must fail before NT submit.
- Post-only limit order with `gtd` must fail in bolt-v3 until an explicit TOML-owned expiry contract is approved.
- GTD without an expiry must not be generated; if exposed, expiry must derive from explicit TOML-controlled timing, not from an unrelated cadence.
- Non-post-only taker entry and exit behavior must remain unchanged.
- Current provisional dirty code is not proof and must be validated against this task list before keep/rework/remove decisions.

## Requirements

### Functional Requirements

- **FR-001**: System MUST derive maker-order support from pinned NautilusTrader Polymarket adapter source.
- **FR-002**: System MUST keep NT naming: `order_type`, `time_in_force`, `is_post_only`, and NT enum semantics.
- **FR-003**: System MUST preserve one config-to-runtime-to-NT-submit path.
- **FR-004**: System MUST expose only NT-supported maker combinations for this strategy slice.
- **FR-005**: System MUST reject unsupported order combinations before submit.
- **FR-006**: System MUST implement new production behavior through TDD, with red evidence before production code changes; already-committed candidate code without red evidence MUST be replayed from a clean base or explicitly waived by the user before completion is claimed.
- **FR-007**: System MUST run adversarial review before implementation and audit review after implementation.
- **FR-008**: System MUST not claim live trade readiness or live smoke coverage unless a real approved live/no-submit/canary artifact proves it.

### Key Entities

- **OrderParams**: TOML strategy order table using NT order fields and flags.
- **MakerOrderScope**: Supported maker combinations proven by pinned NT adapter.
- **GtdExpiryPolicy**: If GTD is exposed later, maps explicit TOML-owned timing into NT `expire_time`.
- **ReviewGate**: Evidence record for internal and external model approvals.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Every supported maker combination has NT source evidence and bolt-v3 source/test evidence.
- **SC-002**: Every task in `tasks.md` is checked off before completion is claimed.
- **SC-003**: Focused and full relevant cargo checks pass on exact branch head before commit/push.
- **SC-004**: External review quorum is recorded or explicitly blocked with reason before any implementation/audit gate is treated as passed.

## Assumptions

- Scope is Polymarket maker limit orders for the existing `binary_oracle_edge_taker` strategy.
- Existing taker entry/exit behavior remains in scope only as regression protection.
- No live submit, transfer, canary, or production deployment is authorized by this spec.
