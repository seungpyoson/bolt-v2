# Research: Global Shadow Execution Policy

## PR #621 Landing Review

PR #621 merged at `dc57c7bb65435d768dc9a4e8cdf6f95ffa91de0a` as "Add session 3 shadow mode".

Relevant landed behavior:

- `parameters.submit_orders` was added to every `config/strategies/binary_oracle_*.toml` strategy file.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` added `ParametersBlock.submit_orders` and raw runtime mapping into the strategy table.
- `src/strategies/binary_oracle_edge_taker/config.rs` added the runtime config field.
- `src/strategies/binary_oracle_edge_taker/mod.rs` added strategy-local branching in `submit_order_with_decision_evidence(...)` and `cancel_resting_order_if_live(...)`.
- `src/bolt_v3_submit_admission.rs` added `evaluate_and_record_without_consuming_capacity(...)`, which is reusable and should remain shared admission behavior.
- `src/shadow_pnl.rs`, `src/bin/shadow_pnl_report.rs`, and `tests/shadow_pnl_report.rs` added offline report behavior that should remain compatible.

The merged PR also fixed important shadow-mode gaps:

- preserve original JSONL line numbers in shadow PnL parse diagnostics
- fail loud on inconsistent or invalid settlement evidence
- suppress forced-flat and external-close cancel mutations in shadow mode
- reject shadow mode with NT-managed venue-action knobs
- tamper-evidence the archetype mapping for submit-order and managed-knob translation

## Current Binding Problem

The safety invariant is correct, but ownership is wrong:

- Mode is owned by strategy `[parameters]`, not root runtime config.
- The operator must edit one value per strategy to switch global runtime behavior.
- The strategy config struct owns a field that is not strategy signal logic.
- The strategy module owns the live/shadow branch around submit and cancel.
- Future strategies would need to rediscover and reimplement the same field, validation, admission behavior, and cancel suppression.

This conflicts with repo rules:

- Rule 7, group by change: switching execution mode must touch one section.
- Rule 9, strategies produce intent only: submit gating must live in shared execution/admission modules built on NT APIs.

## Decision 1: Root Runtime Field

**Decision**: Put live/shadow mode in root `[runtime]`, not in each strategy.

**Rationale**: The mode has runtime lifecycle, not strategy lifecycle. It applies to every Bolt-strategy-originated venue mutation and every loaded-strategy NT managed-action knob in this slice, and the operator should change it once.

**Alternatives considered**:

- Keep `parameters.submit_orders` but add helper methods. Rejected because lifecycle remains per-strategy and future strategies still need duplicated config.
- Put mode under `[risk]`. Rejected because shadow mode is broader than risk limits: it suppresses Bolt-strategy-originated venue mutation while preserving evidence.
- Put mode under each `[clients.<id>.execution]`. Rejected because the operator goal is process-wide shadow mode, and multiple execution clients would reintroduce multi-touch changes.

## Decision 2: Shared Order Execution Module

**Decision**: Add `src/bolt_v3_order_execution.rs` to own execution mode, submit context, submit routing, cancel routing, and the source-fenced strategy-originated venue-mutation chokepoint.

**Rationale**: `src/bolt_v3_order_intent.rs` intentionally stops at NT `OrderFactory -> OrderAny`. `src/bolt_v3_submit_admission.rs` owns admission math and state. A separate execution-policy module can bridge evidence, admission, policy, and the final NT mutation without contaminating order construction. The current production strategy only calls `submit_order(...)` and `cancel_order(...)`, but the policy boundary must also prevent future direct calls to other NT mutation methods.

**Alternatives considered**:

- Extend `bolt_v3_order_intent.rs`. Rejected because the existing order-intent spec explicitly keeps submit/admission/policy out of that module.
- Extend only `bolt_v3_submit_admission.rs`. Rejected because cancel suppression is not submit admission, and combining them would blur admission with execution routing.
- Wrap NT adapters or risk engine. Rejected because PR #621 needs decision evidence and would-be-trade admission evidence before suppression, and NT-managed cancel paths can bypass a submit-only guard.

## Decision 2A: Source-Fence All Strategy-Originated NT Venue Mutations

**Decision**: Add a source-fence/static verifier that rejects direct production strategy calls to NT strategy mutation APIs outside `src/bolt_v3_order_execution.rs`.

**Pinned NT evidence**: NautilusTrader rev `7c2aafb30fb143069c915a3f2057bb12174405f6` exposes these strategy-callable mutation methods in `crates/trading/src/strategy/mod.rs`: `submit_order`, `submit_order_list`, `modify_order`, `cancel_order`, `cancel_orders`, `cancel_all_orders`, `close_position`, and `close_all_positions`.

**Current Bolt evidence**: A repository search finds production strategy calls only at `src/strategies/binary_oracle_edge_taker/mod.rs`: `self.submit_order(...)` and `self.cancel_order(...)`. This slice therefore implements shared submit and cancel routing, but the verifier must already reject every listed method so future strategies cannot bypass the shared policy by using submit-list, modify, batch cancel, cancel-all, or close helpers directly.

**Rationale**: A helper contract without a source-fence is convention-only. The repo already uses source-fence/static verification for architecture boundaries, so the durable enforcement belongs in that same verification layer.

**Alternatives considered**:

- Implement wrappers for all eight NT methods immediately. Rejected for this slice because six methods have no production call site; adding live behavior for unused paths would create speculative API surface.
- Scope the invariant to current submit/cancel call sites only. Rejected because it would let the next strategy reintroduce strategy-bound execution policy through another NT mutation method.

## Decision 3: StrategyBuildContext Carries Policy

**Decision**: Construct one shared policy from root TOML and pass it through `StrategyBuildContext`.

**Rationale**: `StrategyBuildContext` already carries shared fee provider, decision evidence, submit admission, execution venue, and realized-volatility runtime into strategies. Execution policy has the same lifecycle and avoids per-strategy config plumbing.

**Alternatives considered**:

- Store mode in each strategy's raw runtime table. Rejected because that repeats PR #621's binding problem.
- Use a global singleton. Rejected because it makes tests and future multi-runtime construction brittle.

## Decision 4: Shared Validation for NT-Managed Venue Actions

**Decision**: When root execution mode is shadow, validate every loaded strategy and reject NT-managed venue-action knobs.

**Rationale**: `manage_stop`, `manage_gtd_expiry`, `manage_contingent_orders`, and `external_order_claims` can cause NT to mutate venue state outside the explicit shared helpers. This is a process-wide shadow invariant for loaded Bolt strategies, so validation must be shared.

**Pinned NT `StrategyConfig` audit**: NautilusTrader rev `7c2aafb30fb143069c915a3f2057bb12174405f6` defines `StrategyConfig` in `crates/trading/src/strategy/config.rs`. The independent venue-mutation enablers are:

- `external_order_claims`: associates external venue orders with the strategy and therefore can pull venue state into strategy ownership.
- `manage_contingent_orders`: lets NT manage OTO/OCO/OUO open contingent orders automatically.
- `manage_gtd_expiry`: reactivates GTD timers and can emit cancel behavior outside Bolt's shared helper.
- `manage_stop`: triggers cancel-all and close-position market-exit behavior during stop.

The remaining fields are not independent venue-mutation enablers for shadow validation:

- `strategy_id`, `order_id_tag`, `use_uuid_client_order_ids`, and `use_hyphens_in_client_order_ids` affect identity and client-order-ID formatting only.
- `oms_type` changes position accounting semantics in the execution engine but does not emit a venue command by itself.
- `market_exit_interval_ms`, `market_exit_max_attempts`, `market_exit_time_in_force`, and `market_exit_reduce_only` only affect the market-exit path enabled by `manage_stop`; rejecting `manage_stop = true` disables their mutation path.
- `log_events`, `log_commands`, and `log_rejected_due_post_only_as_warning` affect logging only.

If NT adds a new `StrategyConfig` field in a future pinned revision, the schema-current/source-fence verifier must fail until this audit is updated and the field is classified.

**Alternatives considered**:

- Keep guard in `binary_oracle_edge_taker` raw mapping. Rejected because it is not reusable and could miss another strategy.
- Allow the knobs and rely on tests. Rejected because the invariant would depend on strategy reachability rather than structure.

## Decision 5: Preserve Shadow PnL Evidence

**Decision**: Shadow submit routing must continue recording order intent and admission decisions without consuming live capacity.

**Rationale**: PR #621's PnL tool is driven by admitted-entry decision evidence. A global policy that simply blocks before admission would destroy the evidence contract.

**Alternatives considered**:

- Block shadow orders before admission. Rejected because it prevents would-be-trade PnL.
- Record a new shadow-only evidence kind. Rejected for this slice because admission decisions already represent the needed would-be-trade stream.
