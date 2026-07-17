# Sealed Strategy Preparation And Shared Batch Registration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax as an execution checklist; commits and exact-head evidence, not this durable plan, record completion.

**Goal:** Remove raw client/config reachability and direct commit methods from strategy callbacks while making one atomic prepared-batch coordinator the only registration path for Live and the Backtester production-registry branches changed by this PR.

**Architecture:** Shared preflight resolves each configured client identity once, retains only safe client-to-venue routes, and copies the non-client root values needed by raw mapping into an immutable snapshot. `StrategyRegistry` is the only producer of opaque prepared production-registry strategies; one public batch coordinator performs NT identity preparation, batch conflict checks, and final commits for Live and the affected Backtester branches.

**Tech Stack:** Rust 2024, NautilusTrader Rust APIs, TOML-backed Bolt-v3 configuration, `anyhow`, Rust integration tests, source-token structural fences, GitHub Actions/Ubicloud remote Rust verification.

## Global Constraints

- Follow `AGENTS.md`; no hardcoded runtime identities, fallbacks, compatibility adapters, dual registration paths, warning suppressions, or deferred debt.
- `StrategyRegistrationContext` must not store or expose `LoadedBoltV3Config`, `BoltV3RootConfig`, `ClientBlock`, or `ResolvedBoltV3Secrets`.
- Live and the `backtesting-vertical-slice` production-registry branches changed by this PR must use the same prepared-batch coordinator; those Backtester branches normally supply a batch of one.
- `PreparedStrategyRegistration` may be named publicly as an opaque return type, but its constructor, identity-preparation operation, strategy accessor, and commit operation are not public.
- Settlement remains capability-gated and is resolved from the prepared execution client and venue before fee-provider construction; execution venue is unconditional.
- Local compile-heavy Rust verification is prohibited. Use `cargo fmt`, `just fmt-check`, `just deny`, `just ci-lint-workflow`, Python/static fences, and exact-head remote CI.
- Two Rust Probe runs have already been consumed for this branch. Do not dispatch another probe; use normal exact-head PR CI after the coherent slice is pushed.
- B3 and bucket D remain excluded.

---

### Task 1: Remove the exact-head compiler and lint debris

**Files:**
- Modify: `src/strategies/registry.rs:143-156`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs:1-6`

**Interfaces:**
- Consumes: the existing registry test-only `TestStrategy` and edge-taker production module.
- Produces: test code with `DataActor` in scope and production imports containing only live symbols.

- [ ] **Step 1: Record the existing RED evidence**

Use exact-head CI at `4b03028691fe6d5e8f091ee82e3ed09141edf7db` as the failing baseline:

```text
nextest archive: E0405/E0277 because DataActor is absent in registry tests
clippy: unused RefCell and Rc in binary_oracle_edge_taker/mod.rs
bvs-clippy/archive: removed register_strategy and old raw_taker_config signature
```

Do not run local Rust compilation and do not dispatch another Rust Probe.

- [ ] **Step 2: Restore the test-only trait import**

Inside `src/strategies/registry.rs`'s `#[cfg(test)] mod tests`, add:

```rust
use nautilus_common::actor::DataActor;
```

Keep the production import as `DataActorNative`; do not broaden production imports for a test-only need.

- [ ] **Step 3: Remove only the dead edge-taker imports**

Change the opening import to:

```rust
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};
```

Do not add an `allow` or `expect` attribute.

- [ ] **Step 4: Run permitted static checks**

Run:

```bash
cargo fmt --all -- --check
git diff --check
```

Expected: both exit 0. These checks do not constitute Rust compilation proof.

- [ ] **Step 5: Commit the isolated cleanup**

```bash
git add src/strategies/registry.rs src/strategies/binary_oracle_edge_taker/mod.rs
git commit -m "fix: clear strategy registration compile debris"
```

---

### Task 2: Seal callback inputs behind prepared routes and a safe config snapshot

**Files:**
- Modify: `src/bolt_v3_strategy_registration.rs:118-350`
- Modify: `src/strategies/binary_oracle_edge_taker/archetype.rs:440-680,1132-1151,1539-1585`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs:825-858,4349-4375`
- Test: `tests/bolt_v3_provider_binding.rs:387-613`
- Test: `tests/bolt_v3_strategy_registration.rs:1064-1181`

**Interfaces:**
- Produces: `PreparedStrategyClientRoutes`, `StrategyPreparationConfig`, `prepare_strategy_client_routes`, `StrategyRegistrationContext::prepared_client_routes`, and `StrategyRegistrationContext::preparation_config`.
- Consumes later: Task 3 uses the safe types while migrating Backtester registration; Task 4 token-fences these interfaces.

- [ ] **Step 1: Add failing API and behavior assertions**

Extend `strategy_registration_resolves_settlement_identity_once_and_assembly_uses_cached_proof` so the parsed context fields reject every raw config/client capability:

```rust
for forbidden in [
    "LoadedBoltV3Config",
    "BoltV3RootConfig",
    "ClientBlock",
    "ResolvedBoltV3Secrets",
    "loaded:",
    "resolved:",
] {
    assert!(!context_fields.contains(forbidden), "context retained {forbidden}");
}
```

Add a production-entrypoint regression next to the missing-signal case:

```rust
#[test]
fn invalid_second_resolution_client_fails_before_any_strategy_is_registered() {
    let (error, registered_strategy_ids) =
        production_registration_error_with_invalid_second_edge_strategy(|invalid| {
            invalid.config.resolution_data = invalid.config.signal_data.values().next().cloned();
            invalid
                .config
                .resolution_data
                .as_mut()
                .expect("copied signal data must provide a resolution fixture")
                .data_client_id = ClientId::from("missing_resolution");
        });
    assert!(error.to_string().contains("missing_resolution"));
    assert!(registered_strategy_ids.is_empty());
}
```

The current context-field assertion must fail because `pub loaded` is present. The new runtime test is expected to compile only after the coherent slice reaches remote CI.

- [ ] **Step 2: Add the safe prepared-route types**

In `src/bolt_v3_strategy_registration.rs`, replace the current route struct with:

```rust
#[derive(Clone)]
pub struct PreparedStrategyClientRoutes {
    venues_by_client_id: BTreeMap<ClientId, Venue>,
}

impl PreparedStrategyClientRoutes {
    pub fn venue(&self, client_id: &ClientId) -> Option<Venue> {
        self.venues_by_client_id.get(client_id).copied()
    }
}

struct ResolvedStrategyClientRoutes<'a> {
    prepared: PreparedStrategyClientRoutes,
    execution_client: &'a ClientBlock,
}
```

Keep one private resolver that performs the single deduplicated `loaded.root.clients.get(...)` loop and returns `ResolvedStrategyClientRoutes`. Add the public safe wrapper:

```rust
pub fn prepare_strategy_client_routes(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
) -> Result<PreparedStrategyClientRoutes, BoltV3StrategyRegistrationError> {
    Ok(resolve_strategy_client_routes(loaded, strategy)?.prepared)
}
```

The wrapper must call the same resolver; it must not contain another client-table traversal.

- [ ] **Step 3: Add the non-client root snapshot**

Add this public, data-only type in `src/bolt_v3_strategy_registration.rs`:

```rust
#[derive(Clone, Debug, Default)]
pub struct StrategyPreparationConfig {
    realized_volatility_max_source_age_ms: Option<BTreeMap<String, u64>>,
    gate_provider_max_age_ms: BTreeMap<String, u64>,
    chainlink_feed_instrument_ids: BTreeSet<String>,
}

pub enum PreparedRealizedVolatilitySurface {
    SurfacesAbsent,
    SurfaceUnknown,
    Resolved { max_source_age_ms: u64 },
}
```

Implement `from_root` by copying only:

```rust
pub fn from_root(root: &BoltV3RootConfig) -> Self {
    let realized_volatility_max_source_age_ms =
        root.realized_volatility_surfaces.as_ref().map(|surfaces| {
            surfaces
                .iter()
                .map(|(id, surface)| (id.clone(), surface.policy.max_source_age_ms))
                .collect()
        });
    let gate_provider_max_age_ms = root
        .gate_providers
        .as_ref()
        .into_iter()
        .flat_map(|providers| providers.iter())
        .filter_map(|(id, provider)| {
            provider.freshness.as_ref()?.max_age_ms.map(|age| (id.clone(), age))
        })
        .collect();
    let chainlink_feed_instrument_ids = root
        .chainlink_data_streams
        .as_ref()
        .into_iter()
        .flat_map(|catalog| catalog.feed_bindings.iter())
        .filter_map(|binding| binding.as_table())
        .filter_map(|binding| binding.get(stringify!(instrument_id)))
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect();
    Self {
        realized_volatility_max_source_age_ms,
        gate_provider_max_age_ms,
        chainlink_feed_instrument_ids,
    }
}
```

Expose only narrow queries:

```rust
pub fn realized_volatility_surface(&self, id: &str)
    -> PreparedRealizedVolatilitySurface;
pub fn gate_provider_max_age_ms(&self, id: &str) -> Option<u64>;
pub fn has_chainlink_feed_binding(&self, instrument_id: &str) -> bool;
```

The realized-volatility query must distinguish an absent root section from an
unknown ID in a present section; do not collapse those states into one `None`.

- [ ] **Step 4: Remove loaded config from the callback context**

Construct one `Arc<StrategyPreparationConfig>` beside `StrategyRegistrationRuntimeResources` and clone it into each context. Change the context fields to:

```rust
pub struct StrategyRegistrationContext<'a> {
    pub strategy: &'a LoadedStrategy,
    pub strategy_kind: &'static str,
    pub capabilities: StrategyRuntimeCapabilities,
    pub decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
    pub iv_query_handles: Arc<BoltV3IvQueryHandleRegistry>,
    pub order_execution_policy: BoltV3OrderExecutionPolicy,
    preparation_config: Arc<StrategyPreparationConfig>,
    client_routes: PreparedStrategyClientRoutes,
    execution_venue: Venue,
    fee_provider: Arc<dyn FeeProvider>,
    settlement: Option<StrategyRegistrationSettlementResources>,
    realized_volatility_runtime: Option<Arc<Mutex<RealizedVolSurfaceRuntime>>>,
}
```

Add crate-visible getters returning `&StrategyPreparationConfig` and `&PreparedStrategyClientRoutes`. `new` may consume `loaded` and `resolved` as parameters but must not retain either.

- [ ] **Step 5: Make raw taker mapping accept only safe inputs**

Change the signature to:

```rust
pub fn raw_taker_config(
    strategy: &LoadedStrategy,
    preparation_config: &StrategyPreparationConfig,
    client_routes: &PreparedStrategyClientRoutes,
) -> Result<Value, BinaryOracleEdgeTakerRuntimeConfigError>
```

Replace all three full-config reads:

```rust
let realized_volatility_max_source_age_ms = match preparation_config
    .realized_volatility_surface(realized_volatility_surface_id)
{
    PreparedRealizedVolatilitySurface::SurfacesAbsent => {
        return Err(/* preserve the absent-section typed error */);
    }
    PreparedRealizedVolatilitySurface::SurfaceUnknown => {
        return Err(/* preserve the unknown-ID typed error */);
    }
    PreparedRealizedVolatilitySurface::Resolved { max_source_age_ms } => {
        max_source_age_ms
    }
};

let resolution_venue = client_routes
    .venue(&resolution_data.data_client_id)
    .ok_or_else(/* preserve the current typed error */)?;

let forced_flat_stale_reference_ms = preparation_config
    .gate_provider_max_age_ms(&resolution_provider_id)
    .filter(|value| *value != 0)
    .ok_or_else(/* preserve the current typed error */)?;
```

Change resolution binding validation to query `has_chainlink_feed_binding` rather than accepting `&BoltV3RootConfig`. Delete `forced_flat_stale_reference_ms_from_gate_provider` once it has no callers.

- [ ] **Step 6: Update Live and Backtester raw-mapping callers**

Live callback:

```rust
let raw = raw_taker_config(
    context.strategy,
    context.preparation_config(),
    context.prepared_client_routes(),
)
.map_err(|error| binding_error(&context, error))?;
```

At each Backtester overlay/canonicalization call site:

```rust
let preparation_config = StrategyPreparationConfig::from_root(&loaded.root);
let client_routes = prepare_strategy_client_routes(&loaded, loaded_strategy)
    .context("prepare configured strategy client routes")?;
let raw_config = raw_taker_config(loaded_strategy, &preparation_config, &client_routes)
    .context("build raw taker config from overlaid production config")?;
```

Do not add a Backtester-local client-map lookup closure.

- [ ] **Step 7: Run local static evidence and commit**

Run:

```bash
cargo fmt --all -- --check
git diff --check
just source-fence-static
```

Expected: exit 0; no dependency-direction, provider-leak, or runtime-literal finding.

Commit:

```bash
git add src/bolt_v3_strategy_registration.rs src/strategies/binary_oracle_edge_taker/archetype.rs crates/backtesting-vertical-slice/src/runner.rs tests/bolt_v3_provider_binding.rs tests/bolt_v3_strategy_registration.rs
git commit -m "fix: seal strategy preparation inputs"
```

---

### Task 3: Introduce the sole prepared-batch coordinator and migrate every consumer

**Files:**
- Modify: `src/bolt_v3_strategy_registration.rs:42-98,790-880`
- Modify: `src/strategies/registry.rs:24-73`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs:700-740,879-910`
- Modify: `tests/support/stub_runtime_strategy.rs`
- Modify: `tests/bolt_v3_strategy_registration.rs:780-820,930-980,1185-1315`
- Test: `tests/bolt_v3_provider_binding.rs:615-700`

**Interfaces:**
- Produces: `register_prepared_strategy_batch` and `PreparedStrategyBatchError::failed_index`.
- Consumes: `StrategyRegistry::prepare_strategy` remains the only public producer of `PreparedStrategyRegistration`.

- [ ] **Step 1: Pin the one-consumer API structurally**

Add assertions that the `PreparedStrategyRegistration` impl contains no public methods and that production references to `.prepare_registration(` and `.commit(` exist only inside `register_prepared_strategy_batch`. Add `prepare_registration(` to the commit-loop forbidden list so identity preparation cannot move into the first commit iteration.

- [ ] **Step 2: Make the prepared value opaque**

Change these methods to private or crate-private as required by the registry module:

```rust
impl PreparedStrategyRegistration {
    pub(crate) fn from_strategy<T>(strategy: T) -> Self
    where
        T: Strategy + StrategyNative + DataActorNative + Component + Debug + 'static;

    fn prepare_registration(&mut self, trader: &Trader) -> anyhow::Result<StrategyId>;
    fn commit(self, trader: &Rc<RefCell<Trader>>) -> anyhow::Result<()>;
}
```

Remove the public `strategy_id` getter if it is not required by the coordinator.

- [ ] **Step 3: Add the common batch error and coordinator**

Implement:

```rust
#[derive(Debug)]
pub struct PreparedStrategyBatchError {
    failed_index: usize,
    source: anyhow::Error,
}

impl PreparedStrategyBatchError {
    fn new(failed_index: usize, source: impl Into<anyhow::Error>) -> Self {
        Self {
            failed_index,
            source: source.into(),
        }
    }

    pub fn failed_index(&self) -> usize {
        self.failed_index
    }
}

impl std::fmt::Display for PreparedStrategyBatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "prepared strategy at batch index {} failed: {}",
            self.failed_index, self.source
        )
    }
}

impl std::error::Error for PreparedStrategyBatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub fn register_prepared_strategy_batch(
    trader: &Rc<RefCell<Trader>>,
    mut prepared: Vec<PreparedStrategyRegistration>,
) -> Result<Vec<StrategyId>, PreparedStrategyBatchError>;
```

The function must:

1. immutably borrow the supplied trader once;
2. run `prepare_registration` for every item;
3. reject duplicate strategy IDs and duplicate order-ID tags across the complete batch;
4. drop the immutable borrow;
5. commit each already-prepared item against the same `Rc<RefCell<Trader>>` in input order;
6. return the prepared IDs in input order.

Every error records its batch index. Do not accept separate prepare and commit traders, expose a token, or add rollback/fallback logic.

- [ ] **Step 4: Route Live registration through the coordinator**

After all binding callbacks return, retain a metadata vector in the same order as the prepared values. Call:

```rust
let registered_strategy_ids = register_prepared_strategy_batch(
    node.kernel().trader(),
    prepared_registrations,
)
.map_err(|error| {
    let strategy = prepared_metadata[error.failed_index()];
    binding_error(strategy, error.to_string())
})?;
```

Zip IDs with metadata to build `BoltV3StrategyRegistrationSummary`. Delete the old local identity-preparation and commit loops.

- [ ] **Step 5: Adapt integration-test stubs without reopening the constructor**

In `tests/support/stub_runtime_strategy.rs`, add:

```rust
pub(crate) fn prepare_stub_runtime_strategy(
    strategy_id: &str,
    context: &StrategyBuildContext,
) -> Result<PreparedStrategyRegistration> {
    let mut registry = StrategyRegistry::new();
    registry.register::<StubRuntimeStrategyBuilder>()?;
    registry.prepare_strategy(
        StubRuntimeStrategyBuilder::kind(),
        &toml::toml! { strategy_id = strategy_id },
        context,
    )
}
```

Replace every integration-test call to `PreparedStrategyRegistration::from_strategy` with this registry-backed helper after assembling the callback's `StrategyBuildContext`. Tests must not require public access to the opaque constructor.

- [ ] **Step 6: Migrate both Backtester registration call sites**

Replace `registry.register_strategy(...)` with:

```rust
let prepared = registry
    .prepare_strategy(registry_key, raw_config, &build_context)
    .with_context(|| format!("prepare {registry_key} strategy through production registry"))?;
register_prepared_strategy_batch(engine.kernel().trader(), vec![prepared])
    .with_context(|| format!("register {registry_key} prepared strategy batch"))?;
```

Use the identical coordinator for the binary-oracle-specific branch. Do not add a Backtester adapter or call `Trader::add_strategy` directly.

- [ ] **Step 7: Run permitted evidence and commit**

Run:

```bash
cargo fmt --all -- --check
git diff --check
just source-fence-static
```

Expected: exit 0 and the registry/source fences find no retired `register_strategy` route.

Commit:

```bash
git add src/bolt_v3_strategy_registration.rs src/strategies/registry.rs crates/backtesting-vertical-slice/src/runner.rs tests/support/stub_runtime_strategy.rs tests/bolt_v3_strategy_registration.rs tests/bolt_v3_provider_binding.rs
git commit -m "fix: share atomic strategy batch registration"
```

---

### Task 4: Make the structural evidence resistant to the reviewed bypasses

**Files:**
- Create: `tests/support/rust_source_tokens.rs`
- Modify: `tests/bolt_v3_strategy_substrate_structure.rs:1-140,393-652`
- Modify: `tests/bolt_v3_provider_binding.rs:387-700`
- Modify: `tests/bolt_v3_strategy_registration.rs:1064-1148`

**Interfaces:**
- Produces: shared comment/string-aware Rust tokenization for structural tests.
- Consumes: Task 2 safe route/snapshot APIs and Task 3 common coordinator.

- [ ] **Step 1: Extract the existing tokenizer without changing semantics**

Move `Token`, `tokenize`, `ident_start`, `ident_continue`, `scan_nested_block_comment`, `scan_raw_string`, `scan_quoted_literal`, and `scan_char_or_lifetime` from `bolt_v3_strategy_substrate_structure.rs` into `tests/support/rust_source_tokens.rs` with this interface:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) text: String,
}

pub(crate) fn tokenize(source: &str) -> Vec<Token>;
pub(crate) fn texts(tokens: &[Token]) -> Vec<&str>;
pub(crate) fn count_sequence(tokens: &[Token], needle: &[&str]) -> usize;
```

Import the module in both structural harnesses with:

```rust
#[path = "support/rust_source_tokens.rs"]
mod rust_source_tokens;
```

Preserve the existing tokenizer adversarial self-test for comments, normal/raw strings, chars, lifetimes, and nested block comments.

- [ ] **Step 2: Replace spelling-coupled context visibility checks**

Tokenize the `StrategyRegistrationContext` body and assert that each protected field is preceded directly by its identifier, not any `pub`, `pub(crate)`, or `pub(super)` sequence. Protected fields:

```text
preparation_config
client_routes
realized_volatility_runtime
execution_venue
fee_provider
settlement
```

Also assert the whole context body contains none of these type/field tokens:

```text
LoadedBoltV3Config BoltV3RootConfig ClientBlock ResolvedBoltV3Secrets loaded resolved
```

- [ ] **Step 3: Make ordering and retired-path checks token based**

Using comment/string-stripped tokens:

- require one definition and one constructor call of the route resolver;
- reject both `# [ expect ( clippy :: too_many_arguments ) ]` and `# [ allow ( clippy :: too_many_arguments ) ]`;
- require identity preparation to finish before the coordinator commit loop;
- reject `prepare_registration`, raw mapping, registry lookup, and context construction inside the commit loop;
- scan the complete registry production token stream for `register_strategy`, public `build`, and runtime `register` methods without newline-sensitive strings.

Add decoy comments and string literals containing every required token and prove they do not satisfy the checks.

- [ ] **Step 4: Pin both role-failure atomicity cases**

Keep the production-entrypoint missing-signal regression and the new missing-resolution regression. Both must assert the rendered client ID and `strategy_ids().is_empty()`.

- [ ] **Step 5: Run structural/static evidence and commit**

Run:

```bash
cargo fmt --all -- --check
git diff --check
just source-fence-static
```

Expected: exit 0, including tokenizer self-tests and all source fences.

Commit:

```bash
git add tests/support/rust_source_tokens.rs tests/bolt_v3_strategy_substrate_structure.rs tests/bolt_v3_provider_binding.rs tests/bolt_v3_strategy_registration.rs
git commit -m "test: harden strategy registration fences"
```

---

### Task 5: Align durable documentation and publish exact-head evidence

**Files:**
- Modify: `docs/superpowers/specs/2026-07-17-single-path-strategy-venue-preflight-design.md`
- Modify: `docs/superpowers/plans/2026-07-17-single-path-strategy-venue-preflight.md`
- External: PR #1442 body, exact-head check runs, native review state

**Interfaces:**
- Consumes: all completed production and test changes.
- Produces: one reviewable PR head with durable docs matching the code and no transient status embedded in the PR body.

- [ ] **Step 1: Reconcile docs with final names and ordering**

Search:

```bash
rg -n "context\.loaded|register_strategy|PreparedStrategyRegistration::from_strategy|fee_provider.*settlement|settlement.*fee_provider|TBD|TODO|FIXME" docs/superpowers src tests crates/backtesting-vertical-slice
```

Expected: retired API names appear only in explicit rejection/history text; every normative sequence says settlement before fee provider and prepare-all before the sole coordinator commit.

- [ ] **Step 2: Run all permitted final local evidence**

Run in this order:

```bash
cargo fmt --all -- --check
git diff --check
just fmt-check
just deny
just ci-lint-workflow
just source-fence-static
```

Expected: every command exits 0. If sandbox policy blocks the deny cache or CI-lint loopback fixture, rerun that same governed command with the required sandbox approval; do not bypass its wrapper.

- [ ] **Step 3: Conduct the internal adversarial checklist**

Verify directly:

```text
1. No callback can name or obtain LoadedBoltV3Config, BoltV3RootConfig, ClientBlock, or resolved secrets.
2. Alias execution/signal/resolution roles use one resolver and one venue value per ClientId.
3. PreparedStrategyRegistration has no public construction, prepare, getter, or commit method.
4. Live and the affected Backtester production-registry branches call the same register_prepared_strategy_batch function.
5. The complete batch finishes NT identity checks before any commit.
6. Missing signal and resolution clients leave the trader empty.
7. No fallback, alternate venue, hardcoded runtime identity, direct `add_strategy`, or retired adapter remains in the production-registry registration paths changed by this PR.
8. Source fences use tokens rather than comments/strings or newline-sensitive spellings.
```

Resolve every local finding before publishing.

- [ ] **Step 4: Commit documentation alignment**

```bash
git add docs/superpowers/specs/2026-07-17-single-path-strategy-venue-preflight-design.md docs/superpowers/plans/2026-07-17-single-path-strategy-venue-preflight.md
git commit -m "docs: align sealed registration implementation"
```

- [ ] **Step 5: Publish without waiting on CI**

Run:

```bash
just sandbox-safe-push
gh pr view 1442 --json headRefOid,isDraft,reviewRequests,statusCheckRollup,url
```

Confirm the remote head equals local `HEAD`, the worktree is clean, and the stable PR body describes safe routes/snapshots plus the shared coordinator without embedding the head SHA or transient check status. Per `AGENTS.md`, report the exact head and detach; do not wait on CI and do not dispatch another Rust Probe.

- [ ] **Step 6: Hand off exact-head review requirements**

The reviewer must inspect terminal exact-head primary Clippy, Nextest archive/test, Backtester Clippy/archive/gate, source-fence, and coverage evidence. Do not merge. Native code-owner approval from the configured required reviewer, stale-review dismissal, last-push approval, and review-thread resolution remain mandatory.

---

## Plan Self-Review Checklist

- Every spec control maps to a production task and explicit evidence.
- Live and the affected Backtester production-registry branches have one prepared-batch coordinator and no adapter.
- The context contains no loaded/root/client-block/secrets capability.
- Safe route and config snapshot signatures are consistent across Tasks 2–4.
- Opaque prepared-type visibility is consistent across registry, tests, and Backtester.
- Missing signal and resolution clients both have entrypoint-level zero-mutation tests.
- All source-fence checks ignore comments and string literals.
- No task requires local Rust compilation or a third Rust Probe.
- No placeholder, deferred fix, or excluded B3/D scope is introduced.
