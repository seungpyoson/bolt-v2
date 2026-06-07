# NT-Backed IV Engine Research

## Decision: Cargo-pinned NT capability ledger defines "all"

The IV engine scope is defined by scanning the NautilusTrader checkout pinned in `Cargo.toml`. The ledger must classify every IV/options surface reachable from the Rust binary: model types, greeks helpers, msgbus APIs, data actor methods, data-engine publish paths, option-chain manager, adapter option-greeks support, and custom volatility data reachable through NT custom data.

**Rationale**: The user requires all relevant NT IV/options capabilities. A source-backed ledger makes this requirement testable and prevents drift.

**Alternatives considered**:

- Manual list in prose: rejected because it can miss capabilities.
- Strategy-driven subset: rejected because it narrows NT usage.

## Decision: Runtime subscriptions are config-selected, not globally exhaustive

The engine supports every NT IV/options source type in the ledger, but starts only TOML-configured subscriptions for configured clients, instruments, series, strike ranges, aggregate selectors, and custom data selectors.

**Rationale**: Supporting all capability types is required; subscribing to every possible instrument/series/source is not safe or bounded.

**Alternatives considered**:

- Subscribe to every discoverable option instrument: rejected because cardinality is unbounded.
- Per-strategy direct subscriptions: rejected because it creates duplicate IV mechanics.

## Decision: IV profiles are the TOML lifecycle boundary

One `IvProfile` owns sources, source lifecycle, strategy authorization, enabled products, freshness, retention, interpolation, fallback, quorum, projection, and derived-input policy.

**Rationale**: Repo rules require group-by-change. A source swap must not require editing a source section and a separate strategy allow-list.

**Alternatives considered**:

- Separate `iv.sources` and `iv.strategy_access` sections: rejected because source IDs are repeated and source lifecycle changes become multi-edit changes.
- Per-strategy source blocks: rejected because it duplicates IV subscription mechanics and creates strategy-specific IV paths.

## Decision: IV engine is live-integrated, not an isolated library

The plan includes root TOML loading, crate export, live-node startup/shutdown, NT subscription planning, and strategy-registration query handles.

**Rationale**: The deliverable is an IV engine that subscribes through NT. Unit-testable store code alone would not satisfy the runtime requirement.

**Alternatives considered**:

- Build only a reusable IV library first: rejected because it can pass tests without owning the live NT subscription boundary.
- Let strategies instantiate the engine: rejected because strategy modules would own runtime mechanics.

## Decision: Preserve raw NT payloads and build indexed IV products

The IV store keeps raw NT payloads and indexed IV products. Strategies may query both through the IV engine API.

**Rationale**: Raw preservation satisfies full NT capability access; indexed products give strategies usable IV views without reimplementing state.

**Alternatives considered**:

- Raw-only pass-through: rejected because every strategy would own IV state.
- Indexed-only normalized model: rejected because it discards NT data.

## Decision: Aggregate greeks are a typed product

NT aggregate greeks events are preserved raw and indexed into `IvAggregateGreeks` with source, selector, timestamps, values, and provenance.

**Rationale**: Aggregate greeks were part of the requested NT capability set. Leaving them as raw-only would create a hidden second-class path.

**Alternatives considered**:

- Raw-only aggregate greeks access: rejected because strategies would need custom parsing/state.
- Fold aggregate greeks into `IvGreeksPoint`: rejected because aggregate selectors are not necessarily one instrument-level IV point.

## Decision: Derived IV uses NT helpers only with complete configured inputs

Derived IV is available when the request supplies or resolves all required inputs: option price, underlying price, strike, option side, time-to-expiry, rate, carry, source timestamps, and accepted convention. Missing or invalid inputs reject.

**Rationale**: NT helpers are part of the capability set, but IV derivation is invalid if required assumptions are guessed.

**Alternatives considered**:

- Hardcoded default rate/carry/time convention: rejected by no-hardcodes and correctness.
- Strategy-local derivation: rejected because it bypasses the IV engine.

## Decision: Interpolation, fallback, extrapolation, and quorum are explicit policy objects

Interpolation, fallback, extrapolation, and quorum behavior are TOML-owned typed policies. Query results record policy decisions in provenance. Missing policy, unknown policy, insufficient inputs, no qualifying fallback candidate, or quorum failure rejects.

**Rationale**: These policies can change the returned IV. They must be auditable and fail closed.

**Alternatives considered**:

- Hardcode default interpolation or fallback: rejected by no-hardcodes and because it changes strategy behavior silently.
- Let strategies apply policy after raw queries: rejected because it duplicates IV mechanics and weakens provenance.

## Decision: Custom volatility data is IV evidence, not an option-chain IV point

Custom volatility data from NT adapters is stored as `IvEvidence`. It can be queried directly and can participate in configured projection/fallback policy, but it is not mislabeled as instrument-level option-chain IV.

**Rationale**: Broad volatility data can be useful IV evidence while representing different semantics than per-instrument option IV.

**Alternatives considered**:

- Store custom vol as `IvPoint`: rejected because it loses semantic distinction.
- Ignore custom vol data: rejected because it misses NT capability.

## Decision: NT timestamp handling is explicit

NT timestamps are represented as nanosecond timestamps in the IV engine. Any millisecond view for existing Bolt integration must use a named conversion helper with tests.

**Rationale**: Current Bolt modules often use millisecond fields, while NT IV/options data uses nanosecond timestamps. Silent conversion is a correctness risk.

**Alternatives considered**:

- Convert all timestamps to milliseconds on ingest: rejected because it loses precision.
- Mix integer timestamp units by convention: rejected because tests cannot reliably detect misuse.

## Decision: Lifecycle and retention are first-class

The engine tracks subscription state, unsubscribe, reload, source removal, stale marking, and retention eviction. Removed or stale data cannot satisfy current queries.

**Rationale**: Option-chain surfaces can grow quickly and stale data is dangerous if it appears current.

**Alternatives considered**:

- Keep latest value forever: rejected because removed/stale subscriptions would leak into decisions.
- Let each strategy filter staleness: rejected because it duplicates policy.

## Decision: Strategy IV source-fence is global

Strategy modules cannot import NT IV/options subscription APIs or NT IV/greeks math helpers for strategy-local IV derivation. Strategies consume only the public IV query API.

**Rationale**: Conditional source-fence based on runtime config is difficult to enforce statically and can recreate dual IV paths.

**Alternatives considered**:

- Reject only for strategies configured to use IV: rejected because static source-fence cannot reliably know runtime profile bindings.
- Allow strategy-local helper derivation for edge cases: rejected because the IV engine is the single owner of derived IV provenance.
