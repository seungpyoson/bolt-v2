# Feature Specification: Fee-Provider Binding Decoupling

**Feature Branch**: `codex/453-fee-provider-decoupling`
**Created**: 2026-05-23
**Status**: Draft
**Input**: Issue #453, PR #434 follow-up: decouple `binary_oracle_edge_taker` fee-provider binding from direct Polymarket construction.

## User Scenarios & Testing

### User Story 1 - Generic Fee-Provider Resolution (Priority: P1)

As a maintainer, I need strategy runtime registration to obtain a fee provider through a generic execution-client capability boundary instead of calling `polymarket::build_fee_provider` from the `binary_oracle_edge_taker` archetype.

**Why this priority**: Direct Polymarket construction in archetype registration makes one strategy source file venue-specific and blocks multi-venue registration without source edits.

**Independent Test**: Registration tests prove the archetype resolves a fee provider via a registry or capability boundary, while preserving current Polymarket behavior for existing TOML clients.

**Acceptance Scenarios**:

1. **Given** a loaded strategy with `execution_client_id`, **When** runtime registration builds `StrategyBuildContext`, **Then** fee-provider construction is delegated to a generic provider boundary selected by client config.
2. **Given** current Polymarket config and secrets resolution, **When** registration runs, **Then** the resulting fee provider still warms and serves CLOB fee bps as before.
3. **Given** a future non-Polymarket execution client with a fee-provider binding, **When** the binding is registered, **Then** `binary_oracle_edge_taker` registration source does not need a direct provider-module edit.

### User Story 2 - Shared Layers Stay Venue-Agnostic (Priority: P2)

As a reviewer, I need the decoupling point to stay out of shared order, admission, and strategy-economics layers.

**Why this priority**: The root problem is provider binding, not order-intent semantics. Moving Polymarket or binary-oracle assumptions into shared runtime code would repeat PR #434 process failures.

**Independent Test**: Source fences or targeted grep tests fail if `polymarket::build_fee_provider` reappears in strategy/archetype registration, while allowing provider-specific code to remain in provider modules.

**Acceptance Scenarios**:

1. **Given** shared order/admission modules, **When** source fence runs, **Then** they contain no Polymarket, binary-oracle, up/down, or strategy-specific fee-provider policy.
2. **Given** provider-specific modules, **When** source fence runs, **Then** concrete provider code remains allowed only behind the provider registry/binding edge.

## Edge Cases

- Execution client exists but has no fee-provider capability.
- Execution client has malformed provider-specific execution config.
- Existing Polymarket credentials remain SSM-only and are never displayed.
- Fee provider warm failures remain fail-closed for strategy readiness.
- Future static instrument-fee provider may use NT instrument fee fields without Polymarket CLOB HTTP.

## Requirements

### Functional Requirements

- **FR-001**: System MUST remove direct `polymarket::build_fee_provider` calls from `binary_oracle_edge_taker` archetype runtime registration.
- **FR-002**: System MUST resolve fee-provider construction through a generic execution-client/provider capability boundary.
- **FR-003**: System MUST preserve current Polymarket fee-provider behavior and TOML shape unless the approved plan names a config migration.
- **FR-004**: System MUST keep concrete venue logic in concrete provider modules or registry bindings, not shared order/admission/runtime core.
- **FR-005**: System MUST add regression coverage proving archetype registration uses the generic boundary.
- **FR-006**: System MUST add a deterministic source-fence guard proving files under `src/bolt_v3_archetypes/`, strategy modules under `src/strategies/`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, and `src/bolt_v3_order_intent.rs` cannot call or import concrete Polymarket fee-provider construction directly.
- **FR-007**: System MUST use TDD one behavior at a time before any production behavior change.
- **FR-008**: System MUST cite current Bolt source and pinned NautilusTrader source for fee-provider capability decisions.

### Key Entities

- **FeeProviderResolver**: Generic Bolt boundary that maps loaded execution-client config to `Arc<dyn FeeProvider>`.
- **ProviderBinding**: Concrete provider implementation that may use provider-specific config, secrets, and NT adapter helpers.
- **StrategyBuildContext**: Existing strategy construction context that receives a resolved fee provider plus evidence and admission state.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A red registration/source-fence test fails on current direct Polymarket binding before implementation.
- **SC-002**: Existing Polymarket registration behavior remains green after decoupling.
- **SC-003**: No shared order/admission module imports Polymarket or strategy-specific fee-provider logic.
- **SC-004**: External plan review records source-proven blockers only before implementation.

## Assumptions

- Current main is `7a700fbf8129b04b7c94488880322a1f0df82fc6`.
- Pinned NautilusTrader revision is `7c2aafb30fb143069c915a3f2057bb12174405f6`.
- #451 remains architecture context only; this issue does not extract the admission/submission wrapper.
