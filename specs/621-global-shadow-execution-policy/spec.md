# Feature Specification: Global Shadow Execution Policy

**Feature Branch**: `codex/621-global-shadow-mode`
**Created**: 2026-06-13
**Status**: Implemented; review gate passed and exact-head PR CI passed
**Input**: User request to review merged PR #621 and make the shadow/no-submit behavior global, shared, and no longer bound to `binary_oracle_edge_taker`.

## User Scenarios & Testing

### User Story 1 - One Global Operator Switch (Priority: P1)

As an operator, I need to switch the bolt-v3 runtime between live venue mutation and shadow/no-submit operation in one root TOML location, so a mode change does not require editing every strategy file.

**Why this priority**: PR #621 added the safety behavior but placed `submit_orders` under each strategy's `[parameters]`. That violates grouping by lifecycle: shadow mode is a runtime execution policy, not a strategy signal parameter.

**Independent Test**: A root config with shadow mode causes all loaded strategies to record order intent and admission evidence without emitting Bolt-strategy-originated NT venue mutations.

**Acceptance Scenarios**:

1. **Given** root TOML sets global execution mode to shadow, **When** a strategy builds an entry order, **Then** order-intent evidence and admission-decision evidence are recorded and no Bolt-strategy-originated `SubmitOrder` reaches NT.
2. **Given** root TOML sets global execution mode to live, **When** a strategy builds an admitted order, **Then** the same shared path consumes submit-admission capacity and calls NT `submit_order`.
3. **Given** a strategy file still contains the old `parameters.submit_orders`, **When** config is parsed or validated, **Then** the field is rejected instead of silently creating a second policy path.

### User Story 2 - Shared Strategy Execution Chokepoint (Priority: P2)

As a strategy author, I need a shared execution policy and venue-mutation routing helper so strategies produce intent and strategy-local signal state while execution gating lives outside strategy modules.

**Why this priority**: Repo rule 9 rejects strategy-owned submit mechanics. PR #621 guarded submit and cancel inside `binary_oracle_edge_taker`, which works for that strategy but does not create reusable architecture or prevent future direct calls to other NT mutation APIs.

**Independent Test**: Source inspection, focused tests, and a source-fence verifier show strategies read execution mode only through `StrategyBuildContext` and production source cannot directly call NT venue mutation APIs outside the shared execution module.

**Acceptance Scenarios**:

1. **Given** any strategy receives `StrategyBuildContext`, **When** it submits a compiled NT order through the shared helper, **Then** the helper records evidence, applies submit admission, chooses live versus shadow routing, and returns one typed outcome.
2. **Given** a strategy needs to cancel a resting order, **When** it calls the shared cancel helper, **Then** shadow mode suppresses the NT cancel and live mode allows the existing NT cancel call.
3. **Given** production source tries to call NT mutation APIs directly, **When** source-fence/static verification runs, **Then** it fails unless the call is inside `src/bolt_v3_order_execution.rs`.
4. **Given** the shared execution module is inspected, **When** imports are reviewed, **Then** it contains no strategy, market-family, provider, or venue-specific policy.

### User Story 3 - Fail Closed Around NT-Managed Venue Actions (Priority: P3)

As a reviewer, I need shadow mode to structurally reject NT-managed venue actions that can mutate the venue outside Bolt's explicit shared venue-mutation chokepoints.

**Why this priority**: PR #621 discovered that `manage_stop`, GTD management, contingent-order management, and external order claims can drive NT-managed venue behavior outside the strategy-level submit/cancel guard.

**Independent Test**: Root shadow mode plus any managed venue-action field on any loaded strategy fails config validation with an explicit message.

**Acceptance Scenarios**:

1. **Given** shadow mode and `manage_stop = true`, **When** config is loaded, **Then** validation rejects the strategy before runtime construction.
2. **Given** shadow mode and `manage_gtd_expiry = true` or `manage_contingent_orders = true`, **When** config is loaded, **Then** validation rejects the strategy before runtime construction.
3. **Given** shadow mode and non-empty `external_order_claims`, **When** config is loaded, **Then** validation rejects the strategy before runtime construction.

### User Story 4 - Preserve Shadow PnL Evidence Contract (Priority: P4)

As an operator, I need the PR #621 shadow PnL report to keep working from admitted order evidence after the global policy move.

**Why this priority**: The business value of shadow mode is the would-be-trade evidence stream, not just suppressing venue mutation.

**Independent Test**: A shadow-mode run records admitted order evidence without consuming live order-count capacity, and `shadow_pnl_report` still derives would-be trades from admitted entries.

**Acceptance Scenarios**:

1. **Given** a shadow entry passes admission checks, **When** the shared helper suppresses NT submit, **Then** admission evidence still records `Admitted`.
2. **Given** multiple shadow entries are evaluated, **When** live submit limits are inspected, **Then** shadow evaluations do not consume live order-count capacity.
3. **Given** decision evidence and settlement evidence, **When** `shadow_pnl_report` runs, **Then** it does not require strategy-specific `submit_orders` state.

## Edge Cases

- Shadow mode must reject managed NT venue-action knobs globally before any strategy is built.
- Live mode must preserve PR #621 behavior for evidence ordering: build admission request, record order intent, record admission decision, then call NT submit.
- Shadow mode must still surface admission rejections as errors where current strategy behavior relies on those errors to clear pending state.
- Cancel suppression must apply to both forced-flat pending-entry cancels and external-position-close pending-entry cancels.
- The shadow invariant covers Bolt-strategy-originated NT venue mutations and NT `StrategyConfig` managed-action knobs on loaded strategies. It does not claim to firewall operator/manual exchange activity or adapter-level behavior outside the loaded Bolt strategies.
- The shared module must not become a venue capability matrix or an NT lifecycle reimplementation.

## Requirements

### Functional Requirements

- **FR-001**: System MUST define one global root-TOML execution mode for live versus shadow/no-submit operation.
- **FR-002**: System MUST remove `submit_orders` from strategy `[parameters]` and reject any remaining strategy-local `submit_orders` key.
- **FR-003**: System MUST carry the global execution mode through `StrategyBuildContext` so every strategy receives the same policy object.
- **FR-004**: System MUST provide a shared order-submit routing helper that records order intent, records submit-admission evidence, applies admission, and decides whether to call NT submit based on the global policy.
- **FR-005**: System MUST provide a shared cancel routing helper or shared policy method so strategy code does not branch on strategy-local shadow config before NT cancel calls.
- **FR-006**: System MUST add a source-fence/static verifier that rejects direct production-source calls to NT venue-mutation APIs outside `src/bolt_v3_order_execution.rs`, including current pinned NT Strategy mutation methods, raw adapter wrapper names, and near-neighbor parameterized/in-place variants.
- **FR-007**: System MUST preserve the existing single NT submit path and must not introduce a parallel submit path.
- **FR-008**: System MUST preserve compiled-order-based submit admission from `src/bolt_v3_submit_admission.rs`.
- **FR-009**: System MUST reject shadow mode with `manage_stop`, `manage_gtd_expiry`, `manage_contingent_orders`, or non-empty `external_order_claims` for every loaded strategy, and MUST document why other pinned NT `StrategyConfig` fields are not independent venue-mutation enablers.
- **FR-010**: System MUST keep `src/bolt_v3_order_intent.rs` free of submit, admission, strategy, venue, and shadow-mode policy.
- **FR-011**: System MUST update active schema docs, fixtures, runtime-literal audit classifications, and source-fence tests so the new global policy is documented and guarded.
- **FR-012**: System MUST use red-green TDD for each production behavior change and remote-first Rust verification for compile/test proof.
- **FR-013**: System MUST complete internal adversarial review and Gemini, Grok, and Claude adversarial review with unanimous approval before implementation begins.

### Key Entities

- **OrderExecutionMode**: Root-TOML runtime mode with live venue mutation or shadow/no-submit behavior.
- **OrderExecutionPolicy**: Shared runtime object derived from `OrderExecutionMode` and carried through `StrategyBuildContext`.
- **SubmitContext**: Shared NT submit arguments outside the compiled order: optional `client_id`, optional `position_id`, and optional `params`.
- **SubmitRoutingOutcome**: Shared outcome indicating whether NT submit was performed or suppressed by shadow mode.
- **CancelRoutingOutcome**: Shared outcome indicating whether NT cancel was performed or suppressed by shadow mode.
- **VenueMutationFence**: Source-fence/static rule that prevents production source from bypassing shared execution policy with direct NT venue mutation calls outside the policy module.
- **ManagedVenueActionGuard**: Config validation rule rejecting NT-managed venue-action knobs under shadow mode.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Switching between live and shadow requires editing exactly one root TOML field.
- **SC-002**: Repository search finds no production read of `submit_orders` under `src/strategies/`.
- **SC-003**: Focused tests prove shadow mode emits no Bolt-strategy-originated NT `SubmitOrder` or `CancelOrder` while still recording order-intent and admission evidence.
- **SC-004**: Focused tests prove live mode still emits NT submit through the existing single submit path.
- **SC-005**: Config tests prove shadow mode rejects every NT-managed venue-action knob listed in FR-008.
- **SC-006**: Source-fence/static verification fails on direct production-source calls to known NT venue mutation APIs outside the shared execution module.
- **SC-007**: All four required reviews, including internal self-review, return approval with no unresolved blockers before implementation starts.

## Assumptions

- PR #621's `shadow_pnl_report` and settlement join behavior remain in scope only to preserve compatibility with the new global mode.
- This feature does not add live exchange support, new venues, new order variants, or a venue capability matrix.
- The first implementation slice updates `binary_oracle_edge_taker` as the only production strategy, but the architecture must be strategy-agnostic for the next strategy to use.
- Exact compile/test proof will use the repo's remote-first verification flow after implementation is committed and pushed.
