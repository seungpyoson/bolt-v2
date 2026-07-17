# Single-Path Strategy Venue Preflight Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve every strategy's configured execution venue once during side-effect-free preflight, before any registration callback.

**Architecture:** `StrategyRegistrationContext::new` becomes fallible, resolves one TOML-backed execution client and venue, and builds the fee provider from that exact pair during preflight. Settlement resources derive from the same venue, while shared assembly and raw strategy config builders perform no later execution-venue lookup or conditional fallback.

**Tech Stack:** Rust, NautilusTrader `LiveNode`, existing Bolt-v3 TOML loader, source-structure integration tests, governed GitHub Actions verification.

## Global Constraints

- Production runtime identities come only from TOML; add no venue, client ID, account, currency, default, or alternate-routing literal.
- Construct and validate every registration context before invoking any binding callback.
- Keep preflight side-effect-free and binding metadata inert.
- Use one execution-client lookup, one private stored venue, and one preflight-built fee provider; remove callback rereads, the late lookup, and the settlement/non-settlement venue branch.
- Do not run local compile-heavy Rust verification; use governed local static gates and exact-head remote verification.
- PR #1442 remains limited to #1431 B1/B2/C2; B3 and D remain outside this slice.

---

### Task 1: Pin Non-Settlement Registration Atomicity

**Files:**
- Modify: `tests/bolt_v3_strategy_registration.rs:952-1050`

**Interfaces:**
- Consumes: `register_bolt_v3_strategies_on_node_with_bindings` and `StrategyRuntimeCapabilities`.
- Produces: a regression proving missing execution-client configuration is rejected before callbacks even when `settlement` is `false`.

- [x] **Step 1: Extract the invalid-second-strategy assertion and add the non-settlement case**

Refactor the existing test setup into a helper accepting capabilities, then keep the settlement assertion and add:

```rust
#[test]
fn non_settlement_registration_resolves_every_venue_before_any_binding_callback() {
    assert_invalid_second_execution_client_fails_before_callbacks(
        StrategyRuntimeCapabilities {
            realized_volatility: false,
            settlement: false,
        },
    );
}
```

The helper must retain the real callback that increments `REGISTRATION_CALLBACK_COUNT` and mutates `LiveNode`, then assert a `Binding` error, callback count `0`, and an empty node strategy-ID set.

- [x] **Step 2: Establish the pre-change failure by direct behavior trace**

Inspect the unchanged production path and record that `settlement: false` makes `settlement_resources_for_context` return `Ok(None)`, so preparation succeeds without validating the missing client and the callback is reached. Local Rust execution is prohibited and both allowed probe runs are already consumed; the exact test will execute in governed remote verification after implementation.

- [x] **Step 3: Check test-source syntax and formatting**

Run: `cargo fmt --all && git diff --check`

Expected: success with no formatting errors.

### Task 2: Resolve Venue Once During Context Construction

**Files:**
- Modify: `src/bolt_v3_strategy_registration.rs:63-275`
- Modify: `src/bolt_v3_strategy_registration.rs:665-689`
- Modify: `src/bolt_v3_providers/mod.rs:1334-1366`
- Modify: `src/strategies/binary_oracle_edge_taker/archetype.rs:574-600`
- Modify: `src/strategies/complete_set_arbitrage/archetype.rs:266-285`
- Modify: `tests/bolt_v3_provider_binding.rs:185-455`

**Interfaces:**
- Consumes: `venue_for_client(&BoltV3RootConfig, &str) -> Option<Venue>` and TOML-loaded `execution_client_id`.
- Produces: `StrategyRegistrationContext::new(...) -> Result<StrategyRegistrationContext<'a>, BoltV3StrategyRegistrationError>` with private `execution_venue: Venue` and `fee_provider: Arc<dyn FeeProvider>`.

- [x] **Step 1: Make context construction fallible and store the configured venue**

Add the private field and return type:

```rust
pub struct StrategyRegistrationContext<'a> {
    // existing fields
    execution_venue: Venue,
    settlement: Option<StrategyRegistrationSettlementResources>,
}

pub fn new(/* existing arguments */) -> Result<Self, BoltV3StrategyRegistrationError> {
    let (execution_client, execution_venue) =
        resolve_execution_client(loaded, strategy)?;
    let fee_provider = resolve_fee_provider(
        strategy.config.execution_client_id.as_str(),
        execution_client,
        execution_venue,
        resolved,
    )
    .map_err(|error| binding_error(strategy, error.to_string()))?;
    let settlement = capabilities
        .settlement
        .then(|| {
            resolve_settlement_capability(
                loaded,
                strategy,
                execution_client,
                execution_venue,
                settlement_runtime_sink,
                settlement_recovery,
                settlement_health_transition_emitter,
            )
        })
        .transpose()
        .map_err(|error| binding_error(strategy, error.message()))?;
    Ok(Self {
        // existing fields
        execution_venue,
        fee_provider,
        settlement,
    })
}
```

- [x] **Step 2: Add the single fail-closed client and venue resolver**

```rust
fn resolve_execution_client<'a>(
    loaded: &'a LoadedBoltV3Config,
    strategy: &LoadedStrategy,
) -> Result<(&'a ClientBlock, Venue), BoltV3StrategyRegistrationError> {
    let execution_client_id = strategy.config.execution_client_id.as_str();
    loaded.root.clients.get(execution_client_id).map(|client| (client, client.venue)).ok_or_else(|| {
        binding_error(
            strategy,
            format!(
                "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
            ),
        )
    })
}
```

Change `resolve_fee_provider` to accept the resolved `&ClientBlock` and `Venue`
instead of `LoadedBoltV3Config`; it must not read `root.clients` or `client.venue`.
Construct and store the fee provider before the context is returned.

`binding_error` must build the existing `Binding` variant from loaded strategy metadata. `binding_message` delegates to it.

- [x] **Step 3: Make settlement resolution consume the resolved venue**

Change `resolve_settlement_capability` to accept the already-resolved `&ClientBlock` and `execution_venue: Venue`, return `Result<StrategyRegistrationSettlementResources, StrategyRegistrationSettlementIdentityError>`, and remove its client-map/venue lookup and `ExecutionVenue` error variant. It reads the settlement account from that client object, so settlement preflight cannot reopen the execution route. Account and currency failures remain explicit errors; no runtime identity literals are introduced.

- [x] **Step 4: Remove all callback execution-venue rereads**

Assembly becomes:

```rust
let execution_venue = context.execution_venue;
let fee_provider = context.fee_provider.clone();
let settlement = settlement_resources_for_context(context);
```

Delete `execution_venue_for_context`. Make `settlement_resources_for_context` return `Option<&StrategyRegistrationSettlementResources>` directly. Remove the registration loop's separate settlement validation call because fallible context construction now validates every declared capability before collection succeeds.

Remove the redundant execution-client `venue_for_client` checks from
`raw_taker_config` and `raw_complete_set_config`; registration preflight owns
that validation. Keep signal and resolution data-client checks because they are
distinct configured routes.

- [x] **Step 5: Propagate the fallible constructor**

In the registration preparation iterator, append `?` to `StrategyRegistrationContext::new(...)`. In `tests/bolt_v3_provider_binding.rs`, return or unwrap the constructor result deliberately; missing-client tests must assert the construction error rather than expecting a later assembly error.

- [x] **Step 6: Pin the single dataflow structurally**

Update `strategy_registration_resolves_settlement_identity_once_and_assembly_uses_cached_proof` to assert:

```rust
assert!(context_fields.contains("execution_venue: Venue"));
assert!(context_fields.contains("fee_provider: Arc<dyn FeeProvider>"));
assert!(assembly.contains("let execution_venue = context.execution_venue;"));
assert!(assembly.contains("let fee_provider = context.fee_provider.clone();"));
assert!(!assembly.contains("venue_for_client("));
assert!(!source.contains("fn execution_venue_for_context("));
assert_eq!(source.matches("resolve_execution_client(").count(), 2);
```

Also assert the fee-provider resolver does not read `root.clients` or
`client.venue`, and the two raw config builders do not call `venue_for_client`
for the execution client. Retain the exact settlement helper provenance
assertions and require `resolve_settlement_capability` to receive the
already-resolved execution client and `execution_venue` without reopening
`root.clients`.

- [x] **Step 7: Run governed local checks**

Run: `cargo fmt --all`, `git diff --check`, `just fmt-check`, `just source-fence-static`, and `just deny`.

Expected: every command succeeds. Do not run local Rust tests, builds, or clippy.

### Task 3: Review And Exact-Head Verification

**Files:**
- Modify if required by findings: files already listed above.

**Interfaces:**
- Consumes: completed single-path implementation and regressions.
- Produces: clean internal adversarial review and exact-head remote evidence.

- [x] **Step 1: Conduct internal adversarial review**

Challenge partial-registration ordering, constructor side effects, config-only identity provenance, settlement venue reuse, hidden callbacks, cfg/test decoys, and reintroduced fallback paths. Address every substantive finding before publication.

- [ ] **Step 2: Commit and publish through the governed path**

Run:

```bash
git add src/bolt_v3_strategy_registration.rs src/bolt_v3_providers/mod.rs src/strategies/binary_oracle_edge_taker/archetype.rs src/strategies/complete_set_arbitrage/archetype.rs tests/bolt_v3_strategy_registration.rs tests/bolt_v3_provider_binding.rs docs/superpowers
git commit -m "fix: resolve strategy venues during preflight"
just sandbox-safe-push
```

- [ ] **Step 3: Run exact-head remote verification**

Run: `just verify-remote`

Expected: exact-head `actionlint`, `gate`, `backtester-gate`, and `host-health` evidence is green, with the new behavior and structural regressions passing.

- [ ] **Step 4: Complete native review controls**

Confirm no unresolved review threads, obtain approval from the reviewer whose node ID is `U_kgDOEZMFhA`, re-check live `main` rules, then queue only through `just merge-queue 1442`.
