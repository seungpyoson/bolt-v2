# Bolt-v3 Monolith Findings

Date: 2026-06-02

Branch: `docs/monolith-findings-20260602`

Base: `origin/main` at `2938bc6f6e7553e436f074163a9e5db8b4c56b11`

Scope: findings only. This document records observed code ownership, size,
coupling, hardcode pressure, and dual-path pressure from the monolith review. It
does not propose a decomposition plan or implementation sequence.

## Size Snapshot

| File | Lines | Observation |
| --- | ---: | --- |
| `src/strategies/binary_oracle_edge_taker.rs` | 18,205 | Strategy monolith with config parsing, signal state, market selection, exposure lifecycle, order construction, admission request construction, actual submit, replay/source-proof helpers, and 200+ tests. |
| `src/bolt_v3_live_node.rs` | 5,229 | Runtime monolith covering secret resolution, adapter mapping orchestration, client registration, build paths, production runner, strategy-free runner, and probes. |
| `src/bolt_v3_adapters.rs` | 970 | Shared adapter mapper, but it derives market identity from loaded strategy config before provider mapping. |
| `src/bolt_v3_client_registration.rs` | 438 | Focused shared registration seam around NT `add_data_client` and `add_exec_client`. |
| `src/bolt_v3_providers/mod.rs` | 1,462 | Provider registry plus provider-specific CLOB v2 proof/artifact request types. |
| `src/bolt_v3_providers/polymarket.rs` | 896 | Provider config, secret resolution, adapter mapping, fee-provider construction, and provider exports. |
| `src/bolt_v3_market_families/updown.rs` | 1,770 | Market-family identity, target validation, market selection support, and fair-probability pricing math. |
| `src/bolt_v3_submit_admission.rs` | 537 | Shared admission state for live-submit approval limits, kill switch checks, and mutation diagnostics. |
| `src/bolt_v3_decision_evidence.rs` | 1,283 | Shared evidence writer and schema for strategy order intent and admission outcomes. |
| `src/bolt_v3_validate.rs` | 1,870 | Shared validation dispatcher for root config, clients, target gate providers, strategy references, feed bindings, and rate bounds. |

The inspected files above total 34,611 lines.

## Strategy Monolith Findings

### `src/strategies/binary_oracle_edge_taker.rs`

1. The strategy file owns runtime config shape and validation for the strategy
   archetype.

   Evidence:
   - `BinaryOracleEdgeTakerConfig` is declared inside the strategy file at
     `src/strategies/binary_oracle_edge_taker.rs:305`.
   - `BinaryOracleEdgeTakerBuilder` and its `parse_config` / validation helpers
     start at `src/strategies/binary_oracle_edge_taker.rs:5557` and
     `src/strategies/binary_oracle_edge_taker.rs:5568`.
   - The same file validates order table fields and NT order field shapes around
     `src/strategies/binary_oracle_edge_taker.rs:5752` and
     `src/strategies/binary_oracle_edge_taker.rs:5867`.

2. The strategy owns market selection state and selected-market construction.

   Evidence:
   - `SelectionState`, `CandidateOutcome`, `CandidateMarket`, and
     `RuntimeSelectionSnapshot` are in the strategy file around
     `src/strategies/binary_oracle_edge_taker.rs:407`,
     `src/strategies/binary_oracle_edge_taker.rs:412`,
     `src/strategies/binary_oracle_edge_taker.rs:426`, and
     `src/strategies/binary_oracle_edge_taker.rs:447`.
   - Selection from NT instruments is implemented in strategy-local helpers:
     `selection_snapshot_from_instruments` at
     `src/strategies/binary_oracle_edge_taker.rs:6452`,
     `selection_snapshot_from_entry_decision_source` at
     `src/strategies/binary_oracle_edge_taker.rs:6464`, and
     `select_configured_market_from_instruments` at
     `src/strategies/binary_oracle_edge_taker.rs:6552`.
   - Execution-venue filtering is also in the strategy file:
     `selected_market_on_execution_venue` at
     `src/strategies/binary_oracle_edge_taker.rs:6432` and
     `outcome_on_execution_venue` at
     `src/strategies/binary_oracle_edge_taker.rs:6446`.

3. The strategy owns order-book state, visible liquidity, VWAP/slippage sizing,
   and book subscription bookkeeping.

   Evidence:
   - `OutcomeBookState` starts at
     `src/strategies/binary_oracle_edge_taker.rs:493`.
   - `OutcomeBookState::max_execution_within_vwap_slippage_bps` starts at
     `src/strategies/binary_oracle_edge_taker.rs:614`.
   - The lower-level VWAP/slippage helper `max_execution_within_vwap_limit`
     starts at `src/strategies/binary_oracle_edge_taker.rs:651`.
   - `OutcomePreparedBooks` starts at
     `src/strategies/binary_oracle_edge_taker.rs:715`.
   - Book subscription replacement is implemented by
     `replace_book_subscriptions` at
     `src/strategies/binary_oracle_edge_taker.rs:2302`,
     `selection_book_subscriptions` at
     `src/strategies/binary_oracle_edge_taker.rs:6419`,
     `unsubscribe_missing_books` at
     `src/strategies/binary_oracle_edge_taker.rs:6601`, and
     `subscribe_new_books` at `src/strategies/binary_oracle_edge_taker.rs:6638`.
   - Live book subscriptions use `BookType::L2_MBP` directly at
     `src/strategies/binary_oracle_edge_taker.rs:6647`,
     `src/strategies/binary_oracle_edge_taker.rs:6658`, and
     `src/strategies/binary_oracle_edge_taker.rs:6669`.
   - Trade-flow buffering is also in the strategy file: `SignedTradeFlow`
     starts at `src/strategies/binary_oracle_edge_taker.rs:873`.

4. The strategy owns reference pricing state, fast venue arbitration, realized
   volatility warming, lead venue signal construction, and probability/edge
   evaluation state.

   Evidence:
   - `PricingState` starts at `src/strategies/binary_oracle_edge_taker.rs:956`.
   - Reference quote observation and spike detection live in `PricingState`:
     `observe_reference_quote` at
     `src/strategies/binary_oracle_edge_taker.rs:1366`,
     `detect_reference_spike` at
     `src/strategies/binary_oracle_edge_taker.rs:1442`, and
     `observe_reference_snapshot` at
     `src/strategies/binary_oracle_edge_taker.rs:1489`.
   - Realized-vol selection and source reporting are in strategy-local methods:
     `observe_realized_vol_candidates` at
     `src/strategies/binary_oracle_edge_taker.rs:1542`,
     `selected_realized_vol_for_candidate` at
     `src/strategies/binary_oracle_edge_taker.rs:1568`, and
     `current_realized_vol_source_at` at
     `src/strategies/binary_oracle_edge_taker.rs:1588`.
   - Lead venue signal construction starts at
     `src/strategies/binary_oracle_edge_taker.rs:1603`.
   - Price agreement, gap, uncertainty, EV, side-selection, and sizing helpers
     live in the strategy file around
     `src/strategies/binary_oracle_edge_taker.rs:6913`,
     `src/strategies/binary_oracle_edge_taker.rs:7056`, and
     `src/strategies/binary_oracle_edge_taker.rs:7091`.
   - Fair probability itself routes through market-family dispatch, not a
     second strategy-local fair-probability implementation:
     `current_fair_probability_up_at` calls
     `bolt_v3_market_families::fair_probability_up_for_family` at
     `src/strategies/binary_oracle_edge_taker.rs:2867`, and position fair
     probability does the same at
     `src/strategies/binary_oracle_edge_taker.rs:3744`.

5. The strategy owns entry and exit decision computation, including forced-flat
   predicate evaluation and EV comparison.

   Evidence:
   - `entry_gate_decision_at` starts at
     `src/strategies/binary_oracle_edge_taker.rs:2682`.
   - `active_forced_flat_reasons_at` starts at
     `src/strategies/binary_oracle_edge_taker.rs:2766`.
   - `entry_evaluation_at` starts at
     `src/strategies/binary_oracle_edge_taker.rs:5038`.
   - `exit_submission_decision_at` starts at
     `src/strategies/binary_oracle_edge_taker.rs:3928`.
   - `evaluate_exit_decision` is a strategy-local pure function at
     `src/strategies/binary_oracle_edge_taker.rs:7475`.
   - `evaluate_forced_flat_predicates` is a strategy-local pure function at
     `src/strategies/binary_oracle_edge_taker.rs:7516`.
   - Forced-flat predicates are later tied to forced-exit order choice and
     submission: `exit_order_execution_config` is selected with the forced flag
     at `src/strategies/binary_oracle_edge_taker.rs:3972`, and a resting entry
     can be cancelled before forced exit at
     `src/strategies/binary_oracle_edge_taker.rs:4753`.

6. The strategy owns exposure, recovery, one-position enforcement, pending
   entry, pending exit, unsupported observed exposure, blind recovery, and
   market cooldown state.

   Evidence:
   - Exposure structs and states are declared in the strategy file:
     `OpenPositionState` at `src/strategies/binary_oracle_edge_taker.rs:977`,
     `PendingEntryState` at `src/strategies/binary_oracle_edge_taker.rs:995`,
     `PendingExitState` at `src/strategies/binary_oracle_edge_taker.rs:1019`,
     `ManagedPositionState` at `src/strategies/binary_oracle_edge_taker.rs:1036`,
     and `ExposureState` at `src/strategies/binary_oracle_edge_taker.rs:1120`.
   - Recovery bootstrap is implemented by `bootstrap_recovery_from_cache` at
     `src/strategies/binary_oracle_edge_taker.rs:2415` and
     `bootstrapped_exposure_for` at
     `src/strategies/binary_oracle_edge_taker.rs:2497`.
   - Recovery scopes open-position cache reads by execution venue at
     `src/strategies/binary_oracle_edge_taker.rs:2422` and
     `src/strategies/binary_oracle_edge_taker.rs:2426`.
   - Recovery rechecks instrument venue before adoption at
     `src/strategies/binary_oracle_edge_taker.rs:2502`.
   - One-position invariant enforcement is in
     `enforce_one_position_invariant` at
     `src/strategies/binary_oracle_edge_taker.rs:2588` and
     `report_one_position_invariant_violation` at
     `src/strategies/binary_oracle_edge_taker.rs:2602`.
   - `ExposureOccupancy` is declared at
     `src/strategies/binary_oracle_edge_taker.rs:7150`.
   - Market cooldown and fill lifecycle functions are in the same file:
     `market_in_cooldown` at
     `src/strategies/binary_oracle_edge_taker.rs:2611`,
     `arm_market_cooldown` at
     `src/strategies/binary_oracle_edge_taker.rs:2617`, and
     `record_market_fill` at
     `src/strategies/binary_oracle_edge_taker.rs:2630`.
   - Pending-entry and pending-exit event reduction is in the same file:
     `entry_order_may_remain_working` at
     `src/strategies/binary_oracle_edge_taker.rs:2333`,
     `mark_exit_order_terminal` at
     `src/strategies/binary_oracle_edge_taker.rs:3565`,
     `on_order_filled` at `src/strategies/binary_oracle_edge_taker.rs:5304`,
     and `on_position_closed` at
     `src/strategies/binary_oracle_edge_taker.rs:5479`.
   - Foreign-venue live-event quarantine is handled inside strategy state by
     `quarantine_foreign_venue_event` at
     `src/strategies/binary_oracle_edge_taker.rs:3371`.
   - `MarketLifecycleLedger` starts at
     `src/strategies/binary_oracle_edge_taker.rs:1736`, and retained lifecycle
     ids are computed by `retained_market_lifecycle_ids` at
     `src/strategies/binary_oracle_edge_taker.rs:2655`.

7. The strategy owns order template parsing, NT order construction, submit
   context, and actual submit calls.

   Evidence:
   - `BinaryOracleEdgeTakerOrderConfig` starts at
     `src/strategies/binary_oracle_edge_taker.rs:142`.
   - `ConfiguredNtOrderTemplate` starts at
     `src/strategies/binary_oracle_edge_taker.rs:160`.
   - `SubmitContext` starts at `src/strategies/binary_oracle_edge_taker.rs:222`.
   - Configured entry order construction is in
     `build_configured_entry_order` at
     `src/strategies/binary_oracle_edge_taker.rs:4593`.
   - Configured exit order construction is in
     `build_exit_order_with_execution_config` at
     `src/strategies/binary_oracle_edge_taker.rs:4682`.
   - Entry and exit submit paths are `try_submit_entry_order` at
     `src/strategies/binary_oracle_edge_taker.rs:4895` and
     `try_submit_exit_order` at
     `src/strategies/binary_oracle_edge_taker.rs:4711`.
   - The submit wrapper records evidence, asks admission, and calls
     `self.submit_order(...)` in `submit_order_with_decision_evidence` at
     `src/strategies/binary_oracle_edge_taker.rs:4200`.
   - Entry submission converts sized notional to shares/quote quantity at
     `src/strategies/binary_oracle_edge_taker.rs:4862`, constructs NT quantity
     with `instrument.try_make_qty(..., Some(true))` at
     `src/strategies/binary_oracle_edge_taker.rs:4867`, and repeats quantity
     construction before submit at
     `src/strategies/binary_oracle_edge_taker.rs:4953`.
   - Exit and entry submission construct venue-precision prices around
     `src/strategies/binary_oracle_edge_taker.rs:4736` and
     `src/strategies/binary_oracle_edge_taker.rs:4960`.

8. The strategy owns admission request construction for built NT orders,
   including fee-inclusive notional, quote-quantity valuation, market-style
   ceiling valuation, and lifecycle policy projection.

   Evidence:
   - `submit_admission_request_from_order` starts at
     `src/strategies/binary_oracle_edge_taker.rs:4228`.
   - It parses order quantity and price from the compiled NT order near
     `src/strategies/binary_oracle_edge_taker.rs:4233` and
     `src/strategies/binary_oracle_edge_taker.rs:4241`.
   - It values quote-quantity orders with instrument context and
     `admission_base_notional_from_order` around
     `src/strategies/binary_oracle_edge_taker.rs:4248`.
   - It values market-style base-quantity orders at structural price ceiling
     around `src/strategies/binary_oracle_edge_taker.rs:4308`.
   - It applies `fee_inclusive_admission_notional` around
     `src/strategies/binary_oracle_edge_taker.rs:4326`.
   - It maps strategy intent to `BoltV3SubmitIntentKind` and constructs
     `BoltV3SubmitAdmissionRequest` around
     `src/strategies/binary_oracle_edge_taker.rs:4328`.
   - A test-only duplicate admission helper exists at
     `src/strategies/binary_oracle_edge_taker.rs:7546`.

9. The strategy owns event handling for NT data and order/position lifecycle.

   Evidence:
   - `impl DataActor for BinaryOracleEdgeTaker` starts at
     `src/strategies/binary_oracle_edge_taker.rs:5203`.
   - The implementation handles start/stop/time/quote/book/trade/order/position
     events in one block from `src/strategies/binary_oracle_edge_taker.rs:5204`
     through `src/strategies/binary_oracle_edge_taker.rs:5479`.
   - Foreign-venue quarantine paths are part of this lifecycle surface, with
     targeted tests such as
     `strategy_refuses_foreign_venue_market_even_when_slug_matches_the_target`
     at `src/strategies/binary_oracle_edge_taker.rs:15993`,
     `recovery_bootstrap_quarantines_foreign_venue_position` at
     `src/strategies/binary_oracle_edge_taker.rs:16095`, and
     `bootstrap_recovery_from_cache_ignores_foreign_venue_position` at
     `src/strategies/binary_oracle_edge_taker.rs:16247`.

10. Entry-decision replay/source-proof helpers live inside the strategy file
    after the strategy builder.

    Evidence:
    - `BinaryOracleEntryDecisionEvidenceSource` starts at
      `src/strategies/binary_oracle_edge_taker.rs:5931`.
    - `derive_entry_reference_proofs_from_quote_observations` starts at
      `src/strategies/binary_oracle_edge_taker.rs:5974`.
    - `record_entry_decision_evidence_from_source` starts at
      `src/strategies/binary_oracle_edge_taker.rs:6098`.
    - `register_source_replay_strategy` starts at
      `src/strategies/binary_oracle_edge_taker.rs:6286`.
    - `apply_entry_decision_source_books` starts at
      `src/strategies/binary_oracle_edge_taker.rs:6317`.

11. Source-bound `price_to_beat` evidence appears in multiple runtime/source
    proof paths.

    Evidence:
    - Live source-owned readiness seed application starts at
      `src/strategies/binary_oracle_edge_taker.rs:2034`.
    - Strategy input evidence requires source-bound `price_to_beat` at
      `src/strategies/binary_oracle_edge_taker.rs:4415`.
    - The entry decision source field constant
      `ENTRY_DECISION_PRICE_TO_BEAT_VALUE_FIELD` is declared at
      `src/strategies/binary_oracle_edge_taker.rs:5927`.
    - Offline readiness extraction for `price_to_beat_value` starts at
      `src/strategies/binary_oracle_edge_taker.rs:6223`.
    - Operator artifact JSON also writes `"price_to_beat_value"` at
      `src/bolt_v3_operator_artifacts.rs:2276`.

12. Generic selected-market identity is still represented with
    `polymarket_*` field names in shared evidence and strategy evidence
    construction.

    Evidence:
    - Shared `BoltV3StrategyInputSnapshot` fields include
      `polymarket_condition_id`, `polymarket_market_slug`, and
      `polymarket_question_id` at `src/bolt_v3_decision_evidence.rs:38`.
    - Shared admission/decision evidence includes optional `polymarket_*`
      selected-market fields at `src/bolt_v3_decision_evidence.rs:270`.
    - Strategy input evidence populates those `polymarket_*` fields at
      `src/strategies/binary_oracle_edge_taker.rs:4530`.

13. The strategy wraps typed updown market-family output into strategy-local
    `CandidateMarket` / `OutcomeSide` structures.

    Evidence:
    - `src/bolt_v3_market_families/updown.rs` defines `SelectedUpdownMarket` at
      `src/bolt_v3_market_families/updown.rs:462` and
      `select_market_from_instruments` at
      `src/bolt_v3_market_families/updown.rs:816`.
    - The strategy defines its own `CandidateMarket` at
      `src/strategies/binary_oracle_edge_taker.rs:412`.
    - The strategy converts selected market-family output into
      `CandidateMarket` in `select_configured_market_from_instruments` at
      `src/strategies/binary_oracle_edge_taker.rs:6552`.
    - The strategy defines a separate `OutcomeSide` enum at
      `src/strategies/binary_oracle_edge_taker.rs:6884`.

14. The strategy test surface is embedded in the same 18,205-line file.

    Evidence:
    - `rg "#[test]" src/strategies/binary_oracle_edge_taker.rs` reports 229
      tests.
    - Tests cover config parsing, pricing, selection, order construction,
      admission, lifecycle, recovery, venue quarantine, fee readiness, source
      evidence, and entry/exit decisions in the same file.

## Live Node, Adapter, and Strategy-Free Findings

### `src/bolt_v3_live_node.rs`

1. The module documentation itself lists many responsibilities in one module:
   forbidden credential env-var blocklist, SSM resolution, adapter mapping,
   client registration, `LiveNodeBuilder::build`, runtime capture, logger
   filters, production runner, strategy-free connectivity, and probes.

   Evidence:
   - Module docs start at `src/bolt_v3_live_node.rs:1`.
   - The responsibility list runs through `src/bolt_v3_live_node.rs:39`.

2. Production, strategy-free, data-client probe, and all-configured-client build
   variants live in the same module and share overlapping config/adapter/build
   steps.

   Evidence:
   - `build_bolt_v3_live_node` starts at `src/bolt_v3_live_node.rs:2013`.
   - The retired strategy-free build helpers shared the same adapter setup as
     the production builder.
   - `build_bolt_v3_all_configured_client_mapping_live_node` starts at
     `src/bolt_v3_live_node.rs:2063`.

3. Secret resolution is in live-node, not only in a secret module.

   Evidence:
   - `resolve_bolt_v3_live_node_secrets` starts at
     `src/bolt_v3_live_node.rs:2024`.
   - It checks forbidden credential environment variables and constructs an
     `SsmResolverSession` before calling shared secret resolution.

4. Transport scoping is derived from loaded strategies.

   Evidence:
   - `trade_transport_loaded_config` starts at
     `src/bolt_v3_live_node.rs:2081`.
   - `trade_transport_client_keys` starts at
     `src/bolt_v3_live_node.rs:2111`.
   - It collects each strategy's `execution_client_id` and each configured
     reference data client id.

5. The strategy-free build path maps adapters from a transport config, then clears
   strategies for the node build.

   Evidence:
   - The retired strategy-free transport helper cleared `strategies` before
     building the node.
   - `build_bolt_v3_all_configured_client_mapping_live_node` maps adapters from
     the loaded config before building the node.

6. Client registration, strategy registration, evidence-writer choice, and
   submit-admission state creation converge in
   `build_live_node_with_clients`.

   Evidence:
   - `build_live_node_with_clients` starts at `src/bolt_v3_live_node.rs:3231`.
   - It chooses `NoStrategyDecisionEvidenceWriter` vs
     `JsonlBoltV3DecisionEvidenceWriter`, creates a fresh
     `BoltV3SubmitAdmissionState`, registers clients, builds the NT node,
     and registers strategies.

7. Retired strategy-free runner/probe orchestration lived mostly in live-node,
   while report formatting lived in a separate readiness module.

   Evidence:
   - The retired controlled-connect helper started from live-node.
   - Data-client probe variants start at
     `src/bolt_v3_live_node.rs:2528` and
     `src/bolt_v3_live_node.rs:2549`.

### `src/bolt_v3_adapters.rs`

1. The adapter module declares itself a no-trade boundary.

   Evidence:
   - Module docs state it converts loaded config and resolved SSM secrets into
     provider-owned NT client factory/config assemblies at
     `src/bolt_v3_adapters.rs:1`.
   - The docs state it never registers clients, opens connections, starts an
     event loop, selects markets, constructs orders, or enables submit at
     `src/bolt_v3_adapters.rs:7`.

2. The public mapper derives `MarketIdentityPlan` from loaded strategy TOML
   before provider mapping.

   Evidence:
   - `map_bolt_v3_adapters` starts at `src/bolt_v3_adapters.rs:238`.
   - The docs above it state the entry point derives `MarketIdentityPlan` from
     loaded strategy TOML at `src/bolt_v3_adapters.rs:235`.
   - It calls `market_identity_plan_from_config(loaded)` at
     `src/bolt_v3_adapters.rs:242`.

3. Provider adapter mapping receives a broad context object.

   Evidence:
   - `ProviderAdapterMapContext` includes `root`, `client_key`, `client`,
     `resolved`, `plan`, and `clock` at `src/bolt_v3_providers/mod.rs:136`.
   - `map_bolt_v3_adapters_with_market_identity_and_provider_lookup` passes
     that context into each provider binding at `src/bolt_v3_adapters.rs:275`.

### `src/bolt_v3_client_registration.rs`

1. Client registration is comparatively focused.

   Evidence:
   - Module docs state it translates `BoltV3AdapterConfigs` into NT
     `add_data_client` / `add_exec_client` calls at
     `src/bolt_v3_client_registration.rs:1`.
   - The docs state it does not open a network connection, run the event loop,
     subscribe, select markets, construct orders, or submit at
     `src/bolt_v3_client_registration.rs:9`.
   - `register_bolt_v3_clients` starts at
     `src/bolt_v3_client_registration.rs:99`.

## Provider Findings

### `src/bolt_v3_providers/mod.rs`

1. The provider registry is shared, but the same file also carries
   provider-specific CLOB v2 artifact/request types.

   Evidence:
   - `ProviderBinding` starts at `src/bolt_v3_providers/mod.rs:424`.
   - The registry entries start at `src/bolt_v3_providers/mod.rs:449`.
   - A comment says every `ClobV2*` type and `*_clob_v2_*` function below is
     provider-specific Polymarket CLOB v2 material at
     `src/bolt_v3_providers/mod.rs:178`.
   - CLOB v2 request/materialization structs start around
     `src/bolt_v3_providers/mod.rs:187`,
     `src/bolt_v3_providers/mod.rs:205`,
     `src/bolt_v3_providers/mod.rs:222`, and
     `src/bolt_v3_providers/mod.rs:238`.

2. Provider bindings expose multiple surfaces through one registry entry:
   config validation, secret requirements, credential log modules, forbidden env
   vars, secret resolution, adapter mapping, fee provider construction,
   entry-decision source input collection, and canary-proof artifact collection.

   Evidence:
   - Fields on `ProviderBinding` are declared from
     `src/bolt_v3_providers/mod.rs:424` through
     `src/bolt_v3_providers/mod.rs:445`.
   - The Polymarket binding wires all of these at
     `src/bolt_v3_providers/mod.rs:450`.

### `src/bolt_v3_providers/polymarket.rs` and submodules

1. Polymarket provider code owns secret resolution and shape validation.

   Evidence:
   - `resolve_secrets` starts at `src/bolt_v3_providers/polymarket.rs:503`.
   - It resolves `private_key_ssm_path`, `api_key_ssm_path`,
     `api_secret_ssm_path`, and `passphrase_ssm_path` and validates private key
     and API-secret shapes around `src/bolt_v3_providers/polymarket.rs:508`
     through `src/bolt_v3_providers/polymarket.rs:560`.

2. Polymarket adapter mapping constructs both data and execution NT config
   values.

   Evidence:
   - `map_adapters` starts at `src/bolt_v3_providers/polymarket.rs:642`.
   - `map_data` starts at `src/bolt_v3_providers/polymarket.rs:734`.
   - `map_execution` starts at `src/bolt_v3_providers/polymarket.rs:828`.

3. Polymarket data mapping builds market-slug filters from the adapter
   `MarketIdentityPlan`.

   Evidence:
   - `map_data` calls `build_market_slug_filters_for_client` around
     `src/bolt_v3_providers/polymarket.rs:765`.
   - `build_market_slug_filters_for_client` starts at
     `src/bolt_v3_providers/polymarket.rs:787`.
   - It reads `updown::target_plans(plan)` and filters by execution client id.

4. Polymarket credential resolution and HTTP client construction are repeated
   across provider surfaces.

   Evidence:
   - Fee provider construction resolves `PolymarketSecrets` and creates
     `PolymarketClobHttpClient` in `build_fee_provider` at
     `src/bolt_v3_providers/polymarket.rs:675`.
   - Balance allowance cache sync resolves `PolymarketSecrets` at
     `src/bolt_v3_providers/polymarket/balance_allowance_cache.rs:58` and
     builds auth headers/request behavior around
     `src/bolt_v3_providers/polymarket/balance_allowance_cache.rs:77`.
   - Collateral accounting source materialization resolves
     `PolymarketSecrets` and builds `PolymarketClobHttpClient` around
     `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs:144`
     and `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs:155`.
   - Venue account state source resolves `PolymarketSecrets` and constructs
     `PolymarketClobHttpClient` around
     `src/bolt_v3_providers/polymarket/venue_account_state_source.rs:77` and
     `src/bolt_v3_providers/polymarket/venue_account_state_source.rs:94`.

5. Entry decision source input collection is provider-specific but adjacent to
   operator artifact generation and fee computation.

   Evidence:
   - `collect_entry_decision_source_inputs` starts at
     `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs:77`.
   - It creates a `PolymarketClobPublicClient` around
     `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs:124`.
   - It computes fee bps by instrument id through
     `entry_decision_fee_bps_by_instrument_id` at
     `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs:744`.
   - `effective_taker_fee_bps_from_nt` starts at
     `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs:762`.

6. Several provider submodules contain fixed vendor/proof literals. Some are
   protocol constants or diagnostic field names; this document records their
   location only, not whether each literal violates the runtime hardcode rule.

   Evidence:
   - Data API paths and query constants are declared at
     `src/bolt_v3_providers/polymarket/venue_account_state_source.rs:21`.
   - Fee behavior self-test constants are declared at
     `src/bolt_v3_providers/polymarket/fee_behavior_source.rs:18`.
   - Balance allowance update path is declared at
     `src/bolt_v3_providers/polymarket/balance_allowance_cache.rs:22`.
   - Adapter signing constants are declared at
     `src/bolt_v3_providers/polymarket/adapter_signing_source.rs:22`.
   - Collateral accounting constants are declared at
     `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs:29`.

## Market-family Findings

### `src/bolt_v3_market_families/updown.rs`

1. The module owns updown market identity and target-plan projection.

   Evidence:
   - Module docs state it owns updown market-family identity as
     `MarketIdentityPlan` plus current/next market slug at
     `src/bolt_v3_market_families/updown.rs:1`.
   - `UpdownTargetPlan` starts at
     `src/bolt_v3_market_families/updown.rs:403`.
   - `plan_market_identity` starts at
     `src/bolt_v3_market_families/updown.rs:591`.
   - `target_plans` starts at `src/bolt_v3_market_families/updown.rs:430`.

2. The same market-family module owns selected-market requirement construction.

   Evidence:
   - `selected_market_requirement` starts at
     `src/bolt_v3_market_families/updown.rs:940`.

3. The same market-family module owns pricing math for fair probability.

   Evidence:
   - `fair_probability_up` starts at
     `src/bolt_v3_market_families/updown.rs:1069`.
   - It consumes spot, strike, realized volatility, seconds to market end, and
     `pricing_kurtosis`.

4. Duplicate Up/Down outcome detection in candidate market construction fails
   closed by returning `None`.

   Evidence:
   - `candidate_market_for_slug` starts at
     `src/bolt_v3_market_families/updown.rs:1168`.
   - The outcome-side match returns `None` for duplicate Up or duplicate Down at
     `src/bolt_v3_market_families/updown.rs:1183`.

## Retired Proof and Admission Findings

### `src/bolt_v3_submit_admission.rs`

1. Shared admission previously contained retired proof-claim handling.

   Evidence:
   - The removed fields were part of the retired gate stack and are no longer
     part of the current submit-admission contract.

2. Shared admission references the production strategy's method by name in its
   comments.

   Evidence:
   - The comments around rounded-order notional refer to
     `binary_oracle_edge_taker::submit_admission_request_from_order` at
     `src/bolt_v3_submit_admission.rs:433`.

### `src/bolt_v3_decision_evidence.rs`

1. Shared decision evidence previously included retired proof fields and
   outcomes.

   Evidence:
   - Those fields were removed with the retired gate stack.

## Validation and Runtime-config Findings

### `src/bolt_v3_validate.rs`

1. Validation is a central dispatcher for root config, clients, gate providers,
   strategy targets, archetype validation, reference data, provider references,
   and Chainlink feed binding coverage.

   Evidence:
   - Module docs describe dispatch into market families, archetypes, and
     providers at `src/bolt_v3_validate.rs:14`.
   - `validate_root_only` starts at `src/bolt_v3_validate.rs:144`.
   - It validates clients and gate providers at
     `src/bolt_v3_validate.rs:170` and `src/bolt_v3_validate.rs:171`.
   - `validate_strategies` starts at `src/bolt_v3_validate.rs:1391`.
   - Strategy validation dispatches into market families and archetypes around
     `src/bolt_v3_validate.rs:1442` and
     `src/bolt_v3_validate.rs:1454`.
   - Target gate-provider references and Chainlink feed binding coverage are
     validated at `src/bolt_v3_validate.rs:1461` and
     `src/bolt_v3_validate.rs:1462`.

2. Feed binding coverage validation is strict and operator-facing.

   Evidence:
   - `validate_chainlink_feed_binding_coverage` starts at
     `src/bolt_v3_validate.rs:1573`.
   - Missing feed binding messages are constructed around
     `src/bolt_v3_validate.rs:1593`.
   - Ambiguous duplicate feed binding messages are constructed around
     `src/bolt_v3_validate.rs:1601`.

### `src/bolt_v3_config.rs`

1. Config loading joins root config, strategy files, and validation in one path.

   Evidence:
   - `strategy_files` is a root config field at `src/bolt_v3_config.rs:59`.
   - Strategy file loading loops over `root.strategy_files` at
     `src/bolt_v3_config.rs:545`.
   - Config loading calls `validate_root_only` and `validate_strategies` at
     `src/bolt_v3_config.rs:580` and `src/bolt_v3_config.rs:581`.

## Test and Documentation Findings

1. The reviewed monolith-related Rust files contain a large embedded test
   surface.

   Evidence:
   - `rg "#[test]"` across the reviewed monolith-related Rust files reports
     321 tests.
   - `src/strategies/binary_oracle_edge_taker.rs` alone contains 229 tests.

2. Existing runtime-literal documentation is itself large and includes audited
   literal classifications for many operator/protocol constants.

   Evidence:
   - `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
     contains audited entries for provider constants, operator artifact fields,
     strategy-free connectivity fields, and submit-admission diagnostics.

3. Retired single-submit precondition tests inspected docs and source text for operator
   artifact terms and approval fields.

   Evidence:
   - The removed tests checked required operator artifact terms, schema docs,
     and approval-field bindings.

## External Review Signals Recorded During Investigation

1. Claude's live-node/adapters review agreed that the adapter and registration
   path is mostly shared, and treated strategy-free mode as runner/probe overlay rather
   than a separate adapter implementation.

2. Gemini's live-node/adapters review objected that adapter mapping derives
   `MarketIdentityPlan` from strategy TOML and that strategy-free mode maps adapters
   before clearing strategies.

3. GLM's live-node/adapters review accepted the shared build path as
   config-driven, while still noting provider-context duplication concerns.

4. Gemini's provider review flagged repeated Polymarket execution-config
   parsing, credential materialization, CLOB client construction, and fee math
   across provider surfaces.

5. GLM's provider review did not treat the provider surface as blocking, but
   also observed provider-context duplication.

6. Full external review of the entire
   `src/strategies/binary_oracle_edge_taker.rs` file was not completed in the
   earlier model-review pass because the source packet exceeded external
   reviewer packet limits. Strategy findings in this document are from local
   source inspection.

## Unclassified Observations

1. Some constants are vendor protocol names, schema field names, record kinds,
   diagnostic strings, or unit conversion constants. This document records their
   location but does not classify every literal against the repo's runtime
   hardcode rule.

2. Some modules are already focused seams by declaration and behavior, notably
   `src/bolt_v3_client_registration.rs`. This document still includes them
   because they are adjacent to the live-node/adapter monolith surface.

3. The findings above are current-state observations from
   `2938bc6f6e7553e436f074163a9e5db8b4c56b11`; old PR branches and stale
   review artifacts were not used as current-state proof.
