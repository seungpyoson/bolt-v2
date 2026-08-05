# Feature Specification: NT Order Intent Layer

**Feature Branch**: `codex/maker-order-proof-clean`
**Created**: 2026-05-20
**Status**: Historical — implemented and reconciled by later slices; current `main` is authoritative
**Input**: User goal to understand and implement a systematic, NT-first order intent layer using evidence from pinned NautilusTrader, TDD, Spec Kit plan/tasks, no-mistakes, and intentional multi-agent review.

## User Scenarios & Testing

### User Story 1 - NT-First Order Architecture (Priority: P1)

As a maintainer, I need Bolt's order layer to be the thinnest possible adapter from TOML and strategy decisions into NautilusTrader orders, without reimplementing NT order lifecycle, risk, adapter, or venue behavior.

**Why this priority**: If the boundary is wrong, maker, taker, GTD, short-side, spot, binary option, perpetual, and option support will keep accreting hardcoded local policy instead of using NT.

**Independent Test**: A reviewer can inspect `research.md`, `plan.md`, and `contracts/order-intent-layer.md` and see direct NT source evidence for each retained Bolt responsibility and each rejected local responsibility.

**Acceptance Scenarios**:

1. **Given** the pinned NT checkout, **When** the order path is reviewed, **Then** NT order types, TIF, `OrderFactory`, `OrderInitialized`, risk, execution, and adapter boundaries are cited with file and line evidence.
2. **Given** current Bolt code, **When** the order path is reviewed, **Then** every local narrowing point is cited with file and line evidence.
3. **Given** a proposed Bolt order abstraction, **When** it duplicates NT lifecycle, venue capability, or adapter translation, **Then** the architecture rejects it.

### User Story 2 - One Template For Maker And Taker (Priority: P2)

As a strategy author, I need entry and exit order configuration to describe NT order semantics directly, so maker and taker behavior use the same code path.

**Why this priority**: Maker-only flags or tuple whitelists create a second policy layer and block mixed configurations such as maker entry with taker exit.

**Independent Test**: TOML order tables can express mixed maker/taker entry and exit using currently enabled NT fields, validate through one normalized config path, and build NT `OrderAny` through NT `OrderFactory`.

**Acceptance Scenarios**:

1. **Given** a maker limit entry and taker market exit, **When** config validates, **Then** the same normalized order template path handles both.
2. **Given** a taker entry and maker limit exit, **When** config validates, **Then** the same normalized order template path handles both.
3. **Given** short-side entry and exit contracts for `binary_oracle_edge_taker`, **When** current strategy economics lack short collateral and exit semantics, **Then** config validation rejects the shape with an explicit strategy-economics boundary instead of implying NT order construction support.
4. **Given** a factory-supported NT order variant not yet enabled by Bolt, **When** the goal claims that variant is supported, **Then** there is a positive TDD construction/admission test for that variant.

### User Story 3 - Submit Placement Without Hidden Policy (Priority: P3)

As an operator, I need order submit evidence and admission to remain explicit while NT still owns submit mechanics and adapter legality.

**Why this priority**: NT submit accepts more than `OrderAny`; client, position, and venue-specific params can affect real execution. Hiding those in a maker/taker abstraction would be wrong.

**Independent Test**: Strategy submission builds NT `OrderAny`, carries optional submit context only at the NT submit boundary, records Bolt-derived evidence, runs admission, then calls NT `submit_order`.

**Acceptance Scenarios**:

1. **Given** a compiled order, **When** submit admission runs, **Then** admission is computed from the compiled NT `OrderAny` view, while NT `OrderInitialized` remains the authoritative lifecycle event.
2. **Given** an execution client id, **When** the strategy submits, **Then** the optional client id is supplied to NT submit context only when config requires explicit routing, not embedded in the NT order template.
3. **Given** a venue-specific submit param supported by an NT adapter, **When** Bolt carries it, **Then** it is carried as NT submit params and not converted into global Bolt venue policy.

### User Story 4 - Adapter-Proven Capability (Priority: P4)

As a reviewer, I need venue legality claims to come from NT adapter code, strategy-free smoke, or live-submit evidence, not a Bolt-maintained capability table.

**Why this priority**: NT adapters encode non-uniform semantics for Polymarket, Binance Spot/Futures, Deribit, OKX, Bybit, and Hyperliquid. A global Bolt table would be stale policy.

**Independent Test**: Capability claims in docs and PRs cite NT adapter source or exact smoke artifacts, and no live readiness claim is made without approved strategy-free or live-submit evidence.

**Acceptance Scenarios**:

1. **Given** GTD config, **When** a venue claim is made, **Then** the claim distinguishes NT model validity from adapter-specific venue semantics.
2. **Given** post-only config, **When** a venue claim is made, **Then** the claim cites how that adapter maps post-only.
3. **Given** a live execution claim, **When** evidence is reviewed, **Then** exact strategy-free or live-submit artifacts exist for the exact head.

## Edge Cases

- `position_side` is strategy position-contract metadata, not an NT order field.
- `client_id`, `position_id`, and submit `params` are NT submit context, not NT order template fields.
- GTD requires explicit TOML-owned expiry input before Bolt can build an NT GTD order.
- Market orders must reject GTD before calling panic-style NT factory constructors.
- Required trigger/trailing/display fields must be validated before NT construction only for order variants enabled by the current slice.
- Passive maker exit can rest on the book and must not be reused for forced-flat unless TOML explicitly chooses passive forced-exit behavior.
- NT order variants not reachable through `OrderFactory` single-order methods must not be implemented by bypassing `OrderFactory`; they require NT factory support or a separately approved design.

## Requirements

### Functional Requirements

- **FR-001**: System MUST define a reusable NT order template for maker and taker orders without a maker-only abstraction.
- **FR-002**: System MUST keep `position_side` outside the NT order template and validate it only as a strategy position contract.
- **FR-003**: System MUST compile order templates with NT `OrderFactory` only.
- **FR-004**: System MUST not build a Bolt venue capability matrix for runtime policy.
- **FR-005**: System MUST validate only the NT model crash-prevention invariants needed before enabled order factory calls.
- **FR-006**: System MUST keep submit context separate from order construction: optional `client_id`, optional `position_id`, and optional params belong to NT submit context.
- **FR-007**: System MUST compute admission from the compiled NT order view and record Bolt admission inputs without duplicating NT's authoritative order event.
- **FR-008**: System MUST remove hardcoded entry/exit tuple whitelists that narrow valid strategy position contracts without NT or strategy evidence.
- **FR-009**: System MUST preserve the existing NT submit path and must not add a parallel submit path.
- **FR-010**: System MUST use TDD for each production behavior change: red evidence, minimal green implementation, then verification.
- **FR-011**: System MUST run intentional multi-agent review before and after substantive implementation slices.
- **FR-012**: System MUST not claim live exchange support without exact-head strategy-free or live-submit evidence.
- **FR-013**: System MUST not claim support for a factory-supported NT order variant until a positive construction/admission test proves that variant.

### Key Entities

- **StrategyPositionContract**: Bolt-owned entry/exit position semantics, including long/short coherence and forced-exit behavior.
- **NtOrderTemplate**: Config-owned NT order semantics used by maker and taker flows.
- **OrderBuildInputs**: Runtime strategy facts NT cannot infer, such as instrument, quantity, price, trigger, activation, and client order id.
- **SubmitContext**: NT submit arguments outside the order: optional `client_id`, optional `position_id`, and optional params.
- **OrderIntentEvidence**: Bolt audit evidence for decision and admission inputs, linked to NT lifecycle evidence by `client_order_id`.
- **AdapterProof**: Source or smoke evidence for venue-specific legality claims.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Every retained Bolt responsibility has direct evidence showing NT does not already own it.
- **SC-002**: Every rejected abstraction has direct evidence showing NT already owns that surface or that it would hardcode venue policy.
- **SC-003**: The first implementation slice has a failing integration-style test before production code changes and a green verification after.
- **SC-004**: `tasks.md` remains the working checklist and every completed implementation task has command or source evidence.
- **SC-005**: Multi-agent review findings are resolved or explicitly recorded before completion is claimed.
- **SC-006**: Any unsupported order variant, adapter, strategy-free, or live-submit proof remains listed as residual scope rather than implied support.

## Assumptions

- The current implementation branch remains `codex/maker-order-proof-clean` unless the user requests a new worktree or branch.
- `specs/022-nt-maker-order-scope/` remains the historical maker-order proof slice; this spec owns the broader architecture.
- No live submit, transfer, canary, or production deployment is authorized by this spec.
