# Research: Fee-Provider Binding Decoupling

## Current Head And Scope Evidence

- Repo main and both issue worktrees start at `7a700fbf8129b04b7c94488880322a1f0df82fc6`, which is PR #434 merge commit.
- PR #434 final head is `c1a226d315abfe404e616a5d9d343142b2066263`; GitHub reports merge commit `7a700fbf8129b04b7c94488880322a1f0df82fc6`.
- Issue #453 is open and scopes removing direct Polymarket fee-provider construction from `binary_oracle_edge_taker` archetype registration.
- Issue #451 is architecture context only for this branch.

## Evidence Map

### Bolt Current Path

- `src/bolt_v3_config.rs:38`: root config stores clients in `clients: BTreeMap<String, ClientBlock>`.
- `src/bolt_v3_config.rs:247-254`: each `ClientBlock` owns `venue`, `data`, `execution`, and `secrets`; the existing TOML `venue` field is the provider dispatch key.
- `src/bolt_v3_config.rs:275`: each strategy owns `execution_client_id`.
- `src/bolt_v3_strategy_registration.rs:16-23`: generic runtime binding delegates strategy registration through a `StrategyRuntimeBinding` function pointer.
- `src/bolt_v3_strategy_registration.rs:96-126`: generic strategy registration iterates loaded strategies and calls the concrete binding's `register` function.
- `src/bolt_v3_providers/mod.rs:1-16`: provider module root already owns per-provider client block shape and dispatch responsibility while keeping provider-neutral helpers in core.
- `src/bolt_v3_providers/mod.rs:97-148`: current `ProviderBinding` registry maps provider keys to concrete provider validators, secret resolution, and adapter mapping functions.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:265-305`: the concrete `binary_oracle_edge_taker` runtime binding loads the execution client, calls `polymarket::build_fee_provider(...)` directly, builds `StrategyBuildContext`, and registers the strategy.
- `src/strategies/registry.rs:36-39`: strategy-facing `FeeProvider` interface is already generic: `fee_bps(...)` and `warm(...)`.
- `src/strategies/registry.rs:41-67`: `StrategyBuildContext` already carries `Arc<dyn FeeProvider>` and exposes generic accessors.
- `src/strategies/binary_oracle_edge_taker.rs:1984-2000`: strategy runtime warms fees through `self.context.fee_provider()` and `fee_provider_arc()`.
- `src/strategies/binary_oracle_edge_taker.rs:2836-2842`: entry evaluation reads outcome fee bps through the generic context.
- `src/strategies/binary_oracle_edge_taker.rs:3412-3418`: exit evaluation reads position outcome fee bps through the generic context.
- `src/bolt_v3_providers/polymarket.rs:508-557`: concrete Polymarket provider builder parses execution config, resolves secrets, builds NT Polymarket CLOB HTTP client, and returns `Arc<dyn FeeProvider>`.
- `src/bolt_v3_providers/polymarket/fees.rs:13-35`: concrete fetcher uses NT `PolymarketClobHttpClient::get_fee_rate(...)` and returns `base_fee`.
- `src/bolt_v3_providers/polymarket/fees.rs:44-66`: concrete provider requires Polymarket venue and token-id symbol shape.
- `src/bolt_v3_providers/polymarket/fees.rs:119-166`: concrete provider warms/cache fee bps and implements `FeeProvider`.

### Pinned NautilusTrader Evidence

- Pinned rev: `7c2aafb30fb143069c915a3f2057bb12174405f6`.
- `crates/adapters/polymarket/src/http/clob.rs:434-437`: NT Polymarket client exposes `get_fee_rate(token_id)` over `/fee-rate`.
- `crates/adapters/polymarket/src/http/models.rs:330-337`: NT `FeeRateResponse` returns taker fee rate in basis points as `base_fee`.
- `crates/adapters/polymarket/src/http/parse.rs:123-130`: NT Gamma parse maps Polymarket `feeSchedule.rate` to instrument taker fee and sets maker fee to zero.
- `crates/adapters/polymarket/src/execution/parse.rs:300-310`: NT Polymarket execution parser reads effective taker fee from `InstrumentAny::BinaryOption(bo).taker_fee`, defaulting to zero otherwise.
- `crates/model/src/instruments/binary_option.rs:371-376`: NT binary option instruments expose maker and taker fee fields.

## Reachability Classification

### Current Behavior

- Runtime strategy registration is generic until it enters the `binary_oracle_edge_taker` binding.
- The strategy itself consumes only `FeeProvider`, not Polymarket concrete types.
- Direct Polymarket coupling is concentrated in archetype registration and provider modules.
- Existing Polymarket fee provider depends on SSM-resolved secrets and NT Polymarket CLOB HTTP.

### Latent Risk

Any future venue or market family using the same archetype must edit `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` unless provider construction is moved behind a registry/capability boundary. That creates hidden venue policy in a strategy archetype.

### Future Enablement Requirement

Runtime registration must resolve `Arc<dyn FeeProvider>` from the execution client/provider binding without the archetype naming a concrete provider module.

## Decisions

### Decision: Add Generic FeeProviderResolver Boundary

**Decision**: Introduce a provider resolver selected by loaded execution-client config. The resolver loads `strategy.config.execution_client_id` from `loaded.root.clients`, uses the existing client `venue` field as the provider key, dispatches through the existing `ProviderBinding` registry, and returns `Arc<dyn FeeProvider>`. The archetype receives a resolved provider through generic registration context or a generic resolver call, not through `polymarket::build_fee_provider`.

**Rationale**: `FeeProvider` and `StrategyBuildContext` are already generic. The missing boundary is construction/selection, not strategy fee consumption. Current config already has the needed dispatch data: strategy `execution_client_id` selects a client, and that client's TOML-owned `venue` selects the provider binding; no config migration or alternate provider key is required.

**Alternatives considered**:

- Use NT instrument taker fee only: rejected for current Polymarket behavior because Bolt currently warms CLOB token fee bps via `/fee-rate` and issue asks to preserve current behavior.
- Move fee logic into shared order/admission code: rejected; fee provider is strategy economics/readiness input, not order construction.
- Rewrite strategy economics: rejected as out of scope.

### Decision: Keep Polymarket Concrete Code In Provider Module

**Decision**: Keep `build_fee_provider(...)`, CLOB HTTP, token-id parsing, and Polymarket secrets in `src/bolt_v3_providers/polymarket*`.

**Rationale**: Concrete edges are allowed in provider modules. The violation is direct archetype binding, not provider-specific code existing.

**Secret-resolution boundary**: The generic resolver passes the existing resolved secrets snapshot to the selected concrete provider binding. The concrete Polymarket builder may keep parsing its execution block and resolving provider-specific secret handles from that snapshot; this issue does not pre-resolve raw credential strings in generic code or change the SSM-only secret source.

**Alternatives considered**:

- Delete Polymarket provider: rejected because current behavior must be preserved.
