# Feature Specification: Quote-Quantity SELL Limit Admission

**Feature Branch**: `codex/452-quote-quantity-sell-admission`
**Created**: 2026-05-23
**Status**: Draft
**Input**: Issue #452, PR #434 follow-up: harden quote-quantity SELL limit admission before short-side entries or quote-sized exits become reachable.

## User Scenarios & Testing

### User Story 1 - Conservative Admission Contract (Priority: P1)

As an operator, I need Bolt submit admission to reject or account for quote-quantity SELL limit orders without understating notional when the current bid is above the limit price.

**Why this priority**: The current strategy-local admission math can compute `quote_qty / bid * limit_price`, which is below the submitted quote quantity when `bid > limit_price`. That is safe only while the path is unreachable; it becomes a live admission risk before shorts or quote-sized exits are enabled.

**Independent Test**: A focused regression constructs a quote-quantity SELL limit order with `bid > limit_price`, derives submit admission from the compiled NT order, and proves the notional contract cannot understate the quote quantity.

**Acceptance Scenarios**:

1. **Given** a compiled quote-quantity SELL limit order, required instrument context, and cache quote with `bid > limit_price`, **When** submit admission is derived, **Then** admission notional is at least the quote quantity.
2. **Given** a quote-quantity BUY limit order, **When** submit admission is derived, **Then** current supported BUY behavior remains compatible with pinned NT risk math unless the plan documents a conservative envelope.
3. **Given** a non-quote-quantity order, **When** submit admission is derived, **Then** notional remains `price * quantity`.
4. **Given** an inverse-instrument quote-quantity SELL Limit or StopLimit order, **When** submit admission is derived, **Then** the conservative non-inverse floor is bypassed and the existing NT-derived notional path is preserved.

### User Story 2 - Current Reachability Stays Explicit (Priority: P2)

As a reviewer, I need current reachable paths, latent risk, and future enablement requirements separated so this issue does not silently become #451.

**Why this priority**: #452 is a narrow future-enablement blocker. #451 is related architecture context only unless an approved plan proves generic extraction is prerequisite.

**Independent Test**: Source evidence shows short-side entry contracts and quote-sized exits remain fail-closed today, while the generic admission contract is still specified and tested for future reachability.

**Acceptance Scenarios**:

1. **Given** current `binary_oracle_edge_taker` config validation, **When** a short position contract is configured, **Then** validation rejects it before runtime admission.
2. **Given** current exit or forced-exit config, **When** `is_quote_quantity=true`, **Then** validation or build-time checks reject it before NT order construction.
3. **Given** #451 is related, **When** implementation starts, **Then** no generic wrapper extraction occurs unless the approved plan and user approval make it a prerequisite.

## Edge Cases

- Quote-quantity SELL limit with cache quote missing.
- Quote-quantity SELL stop-limit, because pinned NT risk treats Limit and StopLimit the same for effective price.
- Inverse instruments, where pinned NT skips quote-to-base conversion.
- Market-like quote-quantity orders, where cache quote or trade is used as `last_px`.
- Admission fallback when compiled order price or cache data is unavailable.

## Requirements

### Functional Requirements

- **FR-001**: System MUST define an explicit submit-admission contract for quote-quantity SELL Limit and StopLimit orders where market data can make `effective_price > last_px`.
- **FR-002**: System MUST prevent submit admission from silently using understated notional for the `bid > limit_price` SELL scenario.
- **FR-003**: System MUST keep the fix venue-agnostic, market-agnostic, strategy-agnostic, and free of Polymarket, binary-oracle, up/down, or strategy identity in generic layers.
- **FR-004**: System MUST preserve current supported long-only, non-quote-exit behavior.
- **FR-005**: System MUST not implement #451 generic wrapper extraction unless the plan proves it is prerequisite and the user approves the scope expansion.
- **FR-006**: System MUST use TDD one behavior at a time before any production behavior change.
- **FR-007**: System MUST cite current Bolt source and pinned NautilusTrader source for the admission math and reachability claims.
- **FR-008**: System MUST prove inverse-instrument quote-quantity SELL Limit and StopLimit orders stay on the existing NT-derived notional path and do not receive the non-inverse conservative floor.

### Key Entities

- **CompiledOrderAdmissionInput**: A compiled NT `OrderAny` plus cache/instrument context used to derive Bolt submit admission.
- **QuoteQuantityAdmissionNotional**: The notional submitted to the Bolt live-canary admission gate.
- **ReachabilityClassification**: Current behavior, latent risk, and future enablement status for each path.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A regression for quote-quantity SELL limit with `bid > limit_price` fails before implementation and passes after implementation.
- **SC-002**: Existing quote-quantity BUY and market admission tests continue to pass.
- **SC-003**: The plan names every in-scope path and explicitly marks #451 extraction as out of scope unless separately approved.
- **SC-004**: External plan review records source-proven blockers only before implementation.
- **SC-005**: Helper-level inverse Limit and StopLimit regressions prove inverse quote-quantity notional remains unchanged.

## Assumptions

- Current main is `7a700fbf8129b04b7c94488880322a1f0df82fc6`.
- Pinned NautilusTrader revision is `7c2aafb30fb143069c915a3f2057bb12174405f6`.
- This issue may add a small generic admission helper if needed, but must not move strategy decision policy.
