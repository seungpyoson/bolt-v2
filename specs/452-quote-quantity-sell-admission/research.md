# Research: Quote-Quantity SELL Limit Admission

## Current Head And Scope Evidence

- Repo main and both issue worktrees start at `7a700fbf8129b04b7c94488880322a1f0df82fc6`, which is PR #434 merge commit.
- PR #434 final head is `c1a226d315abfe404e616a5d9d343142b2066263`; GitHub reports merge commit `7a700fbf8129b04b7c94488880322a1f0df82fc6`.
- Issue #452 is open and scopes quote-quantity SELL limit admission hardening before short-side entries or quote-sized exits.
- Issue #451 is open and related, but is architecture context only for this branch unless user approval expands scope.

## Evidence Map

### Bolt Current Path

- `src/strategies/binary_oracle_edge_taker.rs:3745-3762`: `submit_order_with_decision_evidence(...)` records order intent, derives submit admission, calls `submit_admission().admit(&request)`, then calls NT `submit_order(...)`.
- `src/strategies/binary_oracle_edge_taker.rs:3764-3798`: `submit_admission_request_from_order(...)` parses compiled order quantity/price and uses quote-quantity special handling before constructing `BoltV3SubmitAdmissionRequest`.
- `src/strategies/binary_oracle_edge_taker.rs:3800-3813`: `quote_quantity_last_price_for_order(...)` uses order price for Limit/StopLimit by falling through to `order.price()`.
- `src/strategies/binary_oracle_edge_taker.rs:3833-3851`: `quote_quantity_submit_notional(...)` calculates effective base quantity using `effective_price`, then calculates notional using `last_px`.
- `src/strategies/binary_oracle_edge_taker.rs:3853-3875`: `quote_quantity_effective_price_for_order(...)` uses quote tick ask for BUY lower bound and bid for SELL upper bound; SELL Limit/StopLimit becomes `last_px.max(bid_price)`.
- `src/strategies/binary_oracle_edge_taker.rs:3880-3905`: entry orders are compiled from config through `build_nt_order(...)`.
- `src/strategies/binary_oracle_edge_taker.rs:3966-3993`: exit order construction rejects quote quantity before NT factory.
- `src/strategies/binary_oracle_edge_taker.rs:3522-3525`: exit submission decision blocks quote-quantity exits before order construction.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:925-931`: config validation rejects `parameters.exit_order.is_quote_quantity=true`.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:941-946`: config validation rejects `parameters.forced_exit_order.is_quote_quantity=true`.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:970-973`: config validation rejects short-side position contracts.

### Existing Regression Anchors

- `src/strategies/binary_oracle_edge_taker.rs:7098`: `quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy`.
- `src/strategies/binary_oracle_edge_taker.rs:7142`: `quote_quantity_submit_admission_uses_limit_price_when_nt_cache_quote_missing`.
- `src/strategies/binary_oracle_edge_taker.rs:7181`: `quote_quantity_market_submit_admission_uses_nt_cache_quote_ask`.
- `src/strategies/binary_oracle_edge_taker.rs:7238`: `quote_quantity_market_submit_admission_uses_nt_cache_trade_when_quote_missing`.
- `src/strategies/binary_oracle_edge_taker.rs:9837`: `exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used`.
- `src/strategies/binary_oracle_edge_taker.rs:9869`: `exit_quote_quantity_order_build_is_rejected_before_nt_factory`.
- `src/strategies/binary_oracle_edge_taker.rs:10993`: `configured_short_position_contract_is_rejected_until_short_economics_exists`.

### Pinned NautilusTrader Evidence

- Pinned rev: `7c2aafb30fb143069c915a3f2057bb12174405f6`.
- `crates/common/src/factories/order.rs:164-205`: NT `OrderFactory::limit(...)` accepts `quote_quantity` and stores it on the order.
- `crates/model/src/instruments/mod.rs:314-327`: NT base quantity for quote quantity is `quote_quantity / last_price`.
- `crates/model/src/instruments/mod.rs:334-361`: NT notional for non-inverse instruments is `quantity * multiplier * price`.
- `crates/risk/src/engine/mod.rs:1101-1121`: NT risk selects effective price for quote-quantity Limit/StopLimit, including SELL `last_px.max(quote_tick.bid_price)`.
- `crates/risk/src/engine/mod.rs:1123-1127`: NT risk calculates effective base quantity using effective price.
- `crates/risk/src/engine/mod.rs:1160-1161`: NT risk calculates notional using effective quantity and `last_px`, not effective price.

## Reachability Classification

### Current Behavior

- Quote-quantity BUY entry can be reached because entry config permits quote quantity and current strategy uses entry `build_nt_order(...)`.
- Quote-quantity SELL entry via short-side position contract is not reachable through validated runtime config because short-side contracts are rejected.
- Quote-quantity normal exits and forced exits are not reachable through validated runtime config because quote-sized exits are rejected in archetype validation and again before exit order construction.
- The risky math exists in the current submit-admission function, but the specific SELL Limit path is latent for current supported strategy configuration.

### Latent Risk

If a future branch enables short-side entry or quote-sized exit, a quote-quantity SELL Limit/StopLimit can derive base quantity at `max(limit_price, bid)` but price admission notional at `limit_price`. When `bid > limit_price`, admission notional can be lower than submitted quote quantity.

### Future Enablement Requirement

Before enabling shorts or quote-sized exits, Bolt must have an explicit admission contract for quote-quantity SELL Limit/StopLimit: admit at least submitted quote quantity when an admission request is built; fail closed before request construction when parse or instrument context is insufficient.

## Decisions

### Decision: Use Conservative Bolt Admission Envelope

**Decision**: Bolt submit admission should not mirror pinned NT risk exactly for quote-quantity SELL Limit/StopLimit when mirroring can understate live-canary admission notional. The planned contract is `admission_notional >= submitted_quote_quantity` for quote-quantity orders when conservative computation is available, with fail-closed behavior if the required context is missing.

**Rationale**: NT risk checks venue/model risk. Bolt submit admission is a live-canary/operator safety gate and must not approve less notional than the configured quote-size intent for future SELL quote-quantity paths.

**Alternatives considered**:

- Mirror NT exactly: rejected for Bolt admission because current evidence proves the undernotional shape.
- Block all quote-quantity SELL orders permanently: rejected as too broad for future shorts/quote exits.
- Implement #451 generic wrapper first: rejected unless user approves scope expansion; a narrow helper can satisfy #452.

### Decision: Keep #451 Out Of Scope

**Decision**: Do not extract the full generic evidence/admission/submit wrapper in this issue.

**Rationale**: #452 needs a notional contract. Moving evidence/admission/submit sequencing is #451 and would expand branch scope.

**Alternatives considered**:

- Implement #451 first: rejected because user marked #451 context only.
