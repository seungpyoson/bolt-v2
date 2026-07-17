# Atomic Strategy Preparation And Single-Path Client Resolution Implementation Plan

> **Required execution skill:** Use `superpowers:executing-plans` for inline execution. `superpowers:subagent-driven-development` is an optional alternative only if the user explicitly requests delegation.

**Goal:** Make every deterministic strategy-registration failure occur before the first `LiveNode` mutation, resolve every configured client identity once per strategy with alias reuse, and leave NautilusTrader `Trader::add_strategy` as the explicit non-transactional commit boundary.

**Architecture:** Registration becomes a strict prepare/commit pipeline. The shared preflight resolves a deduplicated map of execution, signal, and resolution clients; each runtime binding then maps and builds its concrete strategy without receiving a `LiveNode`; only after every strategy is prepared and every strategy ID is checked does a final loop consume prepared registrations and call NT. The registry exposes one construction path, eliminating the current duplicated `build` and `register` implementations.

**Tech stack:** Rust, NautilusTrader native Rust API, TOML configuration, `anyhow`, integration tests, source-fence/static verification, GitHub Actions remote Rust verification.

**Governing design:** `docs/superpowers/specs/2026-07-17-single-path-strategy-venue-preflight-design.md`

---

## Task 1: Pin the two uncovered failure modes before changing production code

**Files:**

- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_provider_binding.rs`

### Step 1: Add a production-binding atomicity regression

Extend the existing registration fixture helper so the test can load two edge-taker strategies while preserving unique `strategy_instance_id` and `order_id_tag` values. Add a test named:

```rust
#[test]
fn invalid_second_signal_client_fails_before_any_strategy_is_registered() {
    let mut loaded = loaded_registration_fixture();
    let first = edge_taker_strategy(&loaded).clone();
    let mut second = first.clone();
    second.config.strategy_instance_id = "invalid_second_signal_client".to_string();
    second.config.order_id_tag = "invalid-second-signal-client".to_string();
    second
        .config
        .signal_data
        .values_mut()
        .next()
        .expect("edge fixture must declare one signal source")
        .data_client_id = ClientId::from("missing_signal");
    loaded.strategies = vec![first, second];

    let mut node = registration_test_node(&loaded);
    let error = register_bolt_v3_strategies_on_node_with_bindings(
        &mut node,
        &loaded,
        &resolved_test_secrets(&loaded),
        production_runtime_bindings(),
        test_execution_controls(),
        Arc::new(NoopDecisionEvidenceWriter),
    )
    .expect_err("an unknown signal client must fail registration");

    assert!(error.to_string().contains("missing_signal"));
    assert!(
        node.kernel().trader().borrow().strategy_ids().is_empty(),
        "deterministic preparation failure must precede every NT registration"
    );
}
```

Use existing fixture/node/secret constructors when their behavior matches the names above; if they are currently embedded in another test, extract them without changing their behavior.

### Step 2: Add an alias-reuse regression

Add a fixture where the signal role and execution role use the same configured client ID. Exercise the production binding and assert success. Pair it with a source assertion that the owning resolver contains the only registration-path `.clients.get(` call and that the edge runtime preparation body contains none of:

```rust
["root.clients", "venue_for_client(", "execution_account_id("]
```

The behavioral test proves aliasing remains valid; the structural assertion proves it does not cause a second map lookup.

### Step 3: Strengthen the secrets and constructor fences

In the already-scoped `StrategyRegistrationContext` struct-body assertion:

```rust
assert!(!context_fields.contains("ResolvedBoltV3Secrets"));
assert!(!context_fields.contains("resolved:"));
```

In the constructor/preflight assertion, scope the source spans and require:

```rust
assert_eq!(client_map_get_count_in_registration_preflight, 1);
assert!(!constructor_body.contains("root.clients"));
```

The single allowed lookup must belong to the new prepared-client-route resolver, not the context constructor or a strategy callback.

### Step 4: Commit the regression slice

```bash
git add tests/bolt_v3_strategy_registration.rs tests/bolt_v3_provider_binding.rs
git commit -m "test: expose non-atomic strategy preparation"
```

### Step 5: Establish RED evidence remotely

Local compile-heavy Rust commands are prohibited. Run the permitted checks first:

```bash
cargo fmt --all
git diff --check
just fmt-check
just source-fence-static
```

Commit the compiling regression slice, publish with `just sandbox-safe-push`, run `just rust-probe suggest`, and dispatch one smallest-sufficient focused integration-test probe for `invalid_second_signal_client_fails_before_any_strategy_is_registered`.

Expected RED evidence: registration returns an error after the first strategy has already appeared in `Trader::strategy_ids()`.

---

## Task 2: Resolve all configured client roles once during shared preflight

**Files:**

- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/archetype.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_provider_binding.rs`

### Step 1: Introduce a private prepared-client route set

Add a private value owned by `StrategyRegistrationContext`:

```rust
#[derive(Clone)]
struct StrategyRegistrationClientRoutes {
    venues_by_client_id: BTreeMap<ClientId, Venue>,
}

impl StrategyRegistrationClientRoutes {
    fn venue(&self, client_id: &ClientId) -> Option<Venue> {
        self.venues_by_client_id.get(client_id).copied()
    }
}
```

Expose only the narrow lookup required by strategy preparation:

```rust
impl StrategyRegistrationContext<'_> {
    pub(crate) fn prepared_client_venue(&self, client_id: &ClientId) -> Option<Venue> {
        self.client_routes.venue(client_id)
    }
}
```

Do not store `ClientBlock`, credentials, resolved secrets, or alternate config sources in the context.

### Step 2: Replace execution-only lookup with one identity-aware resolver

Replace `resolve_execution_client` with a resolver that:

1. Collects the execution client ID.
2. Collects every `signal_data.*.data_client_id`.
3. Collects `resolution_data.data_client_id` when configured.
4. Deduplicates them in a `BTreeMap<ClientId, BTreeSet<&'static str>>`, where the role set is used only for typed diagnostics.
5. Calls `loaded.root.clients.get(id.as_str())` exactly once per unique identity.
6. Returns the route map plus the execution `ClientBlock` reference needed for fee-provider and settlement derivation.

Use stable configuration field names in diagnostics, not venue/client literals:

```rust
fn resolve_strategy_client_routes<'a>(
    loaded: &'a LoadedBoltV3Config,
    strategy: &LoadedStrategy,
) -> Result<(StrategyRegistrationClientRoutes, &'a ClientBlock), BoltV3StrategyRegistrationError>
```

When execution and signal IDs are identical, the map has one entry and one `.clients.get` operation. There is no conditional fallback and no second execution-client path.

### Step 3: Derive every execution resource from that result

Inside `StrategyRegistrationContext::new`, preserve the required order:

```rust
let (client_routes, execution_client) =
    resolve_strategy_client_routes(loaded, strategy)?;
let execution_venue = client_routes
    .venue(&strategy.config.execution_client_id)
    .expect("the route resolver returns the execution identity on success");
let settlement = capabilities
    .settlement
    .then(|| resolve_settlement_capability(/* same execution_client and venue */))
    .transpose()?;
let fee_provider = resolve_fee_provider(
    strategy.config.execution_client_id.as_str(),
    execution_client,
    execution_venue,
    resolved,
)?;
```

The `expect` is an immediately established local invariant, not validation-at-a-distance or a runtime fallback. If repository lint/fence policy rejects it, have the resolver return `execution_venue` directly alongside the map instead.

### Step 4: Remove edge-taker callback client-map reads

Change `raw_taker_config` to consume the prepared context:

```rust
pub fn raw_taker_config(
    context: &StrategyRegistrationContext<'_>,
) -> Result<Value, BinaryOracleEdgeTakerRuntimeConfigError>
```

Use `context.strategy`, `context.loaded`, and `context.prepared_client_venue(...)`. Replace both `venue_for_client` calls with required reads from the prepared route set. Pass the already-resolved resolution venue into `validate_resolution_data_binding`; retain `loaded.root` only for non-client configuration such as provider/feed metadata.

Update all tests and the runtime binding caller to the new signature. Do not add a compatibility overload.

### Step 5: Verify and commit

```bash
cargo fmt --all
git diff --check
just fmt-check
just source-fence-static
git add src/bolt_v3_strategy_registration.rs src/strategies/binary_oracle_edge_taker/archetype.rs tests/bolt_v3_strategy_registration.rs tests/bolt_v3_provider_binding.rs
git commit -m "fix: preflight all strategy client routes"
```

---

## Task 3: Make strategy construction atomic and eliminate the registry dual path

**Files:**

- Modify: `src/strategies/registry.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `src/strategies/complete_set_arbitrage/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/archetype.rs`
- Modify: `src/strategies/binary_oracle_maker/archetype.rs`
- Modify: `src/strategies/complete_set_arbitrage/archetype.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `src/strategy_bindings.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs`
- Modify: `tests/bolt_v3_provider_binding.rs`
- Modify: affected registry/unit tests under `src/strategies/`

### Step 1: Add one-use prepared registration ownership

In `src/strategies/registry.rs`, add:

```rust
pub struct PreparedStrategyRegistration {
    strategy_id: StrategyId,
    commit: Option<Box<dyn FnOnce(&Rc<RefCell<Trader>>) -> Result<()>>>,
}

impl PreparedStrategyRegistration {
    pub fn strategy_id(&self) -> &StrategyId {
        &self.strategy_id
    }

    pub fn commit(mut self, trader: &Rc<RefCell<Trader>>) -> Result<StrategyId> {
        let commit = self
            .commit
            .take()
            .context("prepared strategy registration was already consumed")?;
        commit(trader)?;
        Ok(self.strategy_id)
    }
}
```

The commit closure owns the already-built concrete strategy. It performs no TOML parsing, client resolution, registry selection, or strategy construction.

### Step 2: Collapse `StrategyBuilder` to one construction method

Replace the duplicated `build`/`register` trait methods with one concrete associated type and one build method:

```rust
pub trait StrategyBuilder: Send + Sync + 'static {
    type Strategy: Strategy
        + StrategyNative
        + DataActorNative
        + Component
        + std::fmt::Debug
        + 'static;

    fn kind() -> &'static str;
    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>);
    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<Self::Strategy>;
}
```

Use a generic registration adapter to turn `B::build(...)` into `PreparedStrategyRegistration`. Derive the `StrategyId` from the built strategy using the same identifier logic the existing concrete `register` methods use, then capture the strategy by value in the commit closure and call `trader.borrow_mut().add_strategy(strategy)`.

Update `StrategyRegistration` to store only:

```rust
prepare: fn(&Value, &StrategyBuildContext) -> Result<PreparedStrategyRegistration>
```

Expose `StrategyRegistry::prepare_strategy`; delete `StrategyRegistry::build`, `StrategyRegistry::register_strategy`, `BoxedStrategy`, and all concrete builder `register` methods. Do not leave aliases or compatibility paths.

### Step 3: Make runtime bindings pure preparation callbacks

Change `StrategyRuntimeBinding` from a `register(&mut LiveNode, context)` callback to:

```rust
pub prepare: for<'a> fn(
    StrategyRegistrationContext<'a>,
) -> Result<PreparedStrategyRegistration, BoltV3StrategyRegistrationError>,
```

Rename each archetype callback to `prepare_runtime_strategy`. Each callback must do all of the following before returning:

1. Map its raw TOML runtime config.
2. Assemble `StrategyBuildContext`.
3. Construct the production registry.
4. Select the configured builder.
5. Build the concrete strategy through `prepare_strategy`.

No callback receives `LiveNode`, `Trader`, or another mutation handle.

### Step 4: Prepare every strategy before the first commit

Change the shared registration function to collect fully prepared values:

```rust
let prepared = loaded
    .strategies
    .iter()
    .map(|strategy| {
        let binding = binding_for(strategy, bindings)?;
        let context = StrategyRegistrationContext::new(/* existing inputs */)?;
        let strategy_instance_id = strategy.config.strategy_instance_id.clone();
        let strategy_archetype = strategy.config.strategy_archetype.clone();
        let prepared = (binding.prepare)(context)?;
        Ok((strategy_instance_id, strategy_archetype, prepared))
    })
    .collect::<Result<Vec<_>, BoltV3StrategyRegistrationError>>()?;
```

Before mutation, reject:

- duplicate `prepared.strategy_id()` values within the batch;
- a prepared ID already present in `node.kernel().trader().borrow().strategy_ids()`.

Only then commit:

```rust
for (strategy_instance_id, strategy_archetype, prepared) in prepared {
    let registered_strategy_id = prepared
        .commit(node.kernel().trader())
        .map_err(|error| /* existing typed binding error with strategy metadata */)?;
    summary.registered.push(/* existing summary record */);
}
```

Document in code that an error returned by NT `add_strategy` is the external commit boundary. Do not add rollback, clone-and-swap, or a second registration path.

### Step 5: Add focused regressions for complete deterministic preparation

Add tests proving:

1. Invalid raw config in the second strategy produces zero registered strategies.
2. A missing second signal client produces zero registered strategies.
3. A signal/execution alias succeeds and uses the single prepared route.
4. Duplicate prepared strategy IDs fail before commit.
5. A strategy ID already present in the trader fails before any new commit.
6. Preparation callbacks receive no `LiveNode`; a counting commit stub remains zero on every preparation failure.

Keep tests on the real registration entrypoint where production behavior is claimed. Stubs may isolate duplicate-ID and commit-count mechanics, but must not replace the production-binding missing-signal regression.

### Step 6: Run the second and final Rust Probe only after the slice is coherent

Run local non-compile checks, commit, publish, and then use the second allowed probe for the smallest integration-test target covering atomic preparation:

```bash
cargo fmt --all
git diff --check
just fmt-check
just source-fence-static
git add src tests
git commit -m "fix: prepare all strategies before registration"
just sandbox-safe-push
just rust-probe suggest
```

Expected GREEN evidence: all preparation-failure regressions pass; the production missing-signal case leaves `Trader::strategy_ids()` empty.

---

## Task 4: Align durable documentation, harden fences, and publish reviewable evidence

**Files:**

- Modify: `docs/superpowers/plans/2026-07-17-single-path-strategy-venue-preflight.md`
- Modify: `docs/superpowers/specs/2026-07-17-single-path-strategy-venue-preflight-design.md` only if implementation reveals a real design correction
- Modify: PR #1442 body only for timeless scope/behavior disclosures

### Step 1: Remove stale ordering and deleted-symbol claims

Ensure this plan is the only active plan for the slice and contains:

- settlement resolution before fee-provider construction;
- no `binding_message` wrapper in shared registration;
- deduplicated execution/signal/resolution client resolution;
- full deterministic preparation before mutation;
- the explicit NT commit boundary.

Do not put current head SHAs or transient CI status in the PR body.

### Step 2: Run all permitted local evidence

```bash
cargo fmt --all
cargo fmt --all -- --check
git diff --check
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
```

Run targeted text checks for stale APIs:

```bash
rg -n "binding_message|register_runtime_strategy|register_strategy\(|BoxedStrategy|fn register\(" src tests docs/superpowers
rg -n "root\.clients|venue_for_client\(" src/strategies
```

Every remaining match must be either an intentional validation/data-client path outside runtime preparation or a test fixture with an explicit assertion. No compatibility implementation may remain.

### Step 3: Conduct internal adversarial review

Review the final diff against these attacks before requesting external review:

- invalid second signal/resolution client;
- same client ID in multiple roles;
- alternate helper that performs a second client-map lookup;
- private secret storage in the context;
- raw config or builder failure after the first commit;
- duplicate batch strategy IDs;
- already-registered strategy ID;
- fallbacks, condition chains, hardcoded runtime IDs, warning suppressions, dead wrappers, and dual APIs.

Fix every confirmed local finding before publishing.

### Step 4: Publish and request exact-head evidence

```bash
git status --short
just sandbox-safe-push
```

Report the exact remote head SHA and detach; do not wait on CI. Before external review or merge, the reviewer must confirm the required exact-head checks are green under the current pre-cutover policy.

### Step 5: Request the required native reviewer only after evidence is clean

Resolve node ID `U_kgDOEZMFhA` to its current login, keep `.github/CODEOWNERS` aligned, request that reviewer, and leave merging to the native human controls. Do not merge or bypass review/ruleset requirements.

---

## Plan self-review checklist

- Every accepted design requirement maps to a production change and explicit evidence.
- No placeholder names such as “similar helper” or “appropriate test” remain.
- The client resolver is the sole registration-path client-map owner and deduplicates aliases by identity.
- Settlement remains capability-gated, while execution venue remains unconditional.
- Every strategy is concretely built before mutation; final commit owns no fallible repository preparation.
- Registry construction has one path; no build/register compatibility layer remains.
- No resolved secrets or raw client blocks are retained in the strategy context.
- The NT `add_strategy` boundary is disclosed as non-transactional; no rollback is implied.
- Verification respects the two-probe limit and the remote-first Rust policy.
