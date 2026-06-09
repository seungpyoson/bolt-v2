# NT-Backed IV Engine Research

## Decision: IV naming owns only implied-volatility vocabulary

Bolt-owned entities, config keys, source kinds, and product names use `IV` or `implied-volatility` terminology. They do not use standalone volatility names because RV and future volatility features need their own vocabulary.

**Rationale**: The IV engine is about implied volatility. Generic volatility names blur ownership with RV and other volatility engines.

**Alternatives considered**:

- Use `volatility` as a shorter umbrella term: rejected because it is ambiguous once RV exists.
- Keep NT adapter wording in Bolt config keys: rejected because external naming should not leak into Bolt's domain boundary when it creates cross-engine ambiguity.

## Decision: Cargo-pinned NT capability ledger defines "all"

The IV engine scope is defined by scanning the NautilusTrader checkout pinned in `Cargo.toml`. The ledger resolves the locked dependency graph with `cargo metadata --locked`, cross-checks NT package source revisions in `Cargo.lock`, locates the Cargo git checkout for that revision, scans known seed families, and performs a whole-checkout public-symbol candidate sweep for IV/options terms. The sweep includes implied-volatility and option-microstructure vocabulary: option, options, greeks, implied, iv, volatility, smile, surface, chain, custom data, strike, expiry, expiration, tenor, moneyness, skew, premium, and vol. Every candidate is classified as supported, unreachable from the Rust binary, not IV/options related after inspection, or explicitly excluded with approved rationale.

**Rationale**: The user requires all relevant NT IV/options capabilities. A source-backed ledger makes this requirement testable and prevents drift.

**Alternatives considered**:

- Manual list in prose: rejected because it can miss capabilities.
- Strategy-driven subset: rejected because it narrows NT usage.
- Handwritten NT revision or local checkout path: rejected because `Cargo.toml` and `Cargo.lock` are the source of truth.
- Seed-family scan only: rejected because new NT IV/options families could appear outside the current model, data actor, data engine, msgbus, option-chain, greeks-helper, adapter, or custom-data paths.

## Decision: Runtime subscriptions are config-selected, not globally exhaustive

The engine supports every NT IV/options source type in the ledger, but starts only TOML-configured subscriptions for configured clients, instruments, series, strike ranges, aggregate selectors, and custom implied-volatility selectors.

**Rationale**: Supporting all capability types is required; subscribing to every possible instrument/series/source is not safe or bounded.

**Alternatives considered**:

- Subscribe to every discoverable option instrument: rejected because cardinality is unbounded.
- Per-strategy direct subscriptions: rejected because it creates duplicate IV mechanics.

## Decision: IV profiles are the TOML lifecycle boundary

One `IvProfile` owns sources, source lifecycle, strategy authorization, enabled products, freshness, retention, memory bounds, interpolation, fallback, quorum, projection, and derived-input policy.

**Rationale**: Repo rules require group-by-change. A source swap must not require editing a source section and a separate strategy allow-list.

**Alternatives considered**:

- Separate `iv.sources` and `iv.strategy_access` sections: rejected because source IDs are repeated and source lifecycle changes become multi-edit changes.
- Per-strategy source blocks: rejected because it duplicates IV subscription mechanics and creates strategy-specific IV paths.

## Decision: Selectors are a typed union

`IvSelector` is a Rust-validated union with distinct variants for option greeks, option chains, aggregate greeks, custom implied-volatility data, smiles, surfaces, and IV evidence.

**Rationale**: Different NT capabilities need different selector fields. Untyped selector maps would let invalid source/query combinations reach runtime.

**Alternatives considered**:

- Use generic selector maps: rejected because validation would be delayed and error-prone.
- Use one selector shape for all products: rejected because option-chain, aggregate, and evidence selectors are not the same domain object.

## Decision: IV engine is live-integrated, not an isolated library

The plan includes root TOML loading, crate export, live-node startup/shutdown, NT data actor/msgbus subscription binding, event routing, source-health updates, and strategy-registration query handles.

**Rationale**: The deliverable is an IV engine that subscribes through NT. Unit-testable store code alone would not satisfy the runtime requirement.

**Alternatives considered**:

- Build only a reusable IV library first: rejected because it can pass tests without owning the live NT subscription boundary.
- Let strategies instantiate the engine: rejected because strategy modules would own runtime mechanics.

## Decision: Preserve raw NT payloads but expose strategy-safe IV products

The IV store keeps raw NT payloads and indexed IV products. Strategies query IV products through `IvQueryHandle`. Raw payload retrieval is limited to audit, replay, and test handles outside strategy modules. Strategy-facing provenance can include raw event IDs but not raw NT payload structs or bytes.

**Rationale**: Raw preservation satisfies full NT capability access inside the engine; indexed products give strategies usable IV views without reimplementing state or bypassing provenance.

**Alternatives considered**:

- Raw-only pass-through: rejected because every strategy would own IV state.
- Indexed-only normalized model: rejected because it discards NT data.
- Let strategies dereference raw payloads directly: rejected because it bypasses source authorization, policy, freshness, and provenance.

## Decision: Provenance is a required schema

Every raw event, indexed product, derived product, projection, policy output, and rejection carries `IvProvenance` with profile, source, selector fingerprint, NT revision evidence, raw event IDs, input IDs, helper identity, typed policy decisions, timestamp units, ingest sequence, subscription generation, source-health state, and reject reason when applicable.

**Rationale**: Raw, indexed, derived, and projected values are not interchangeable. Required provenance makes policy and source decisions auditable.

**Alternatives considered**:

- Free-form provenance maps: rejected because they would leave audit gaps.
- Provenance only on successful products: rejected because rejected queries also need source and policy evidence.

## Decision: Policy decisions are typed, not strings

Projection, interpolation, extrapolation, fallback, quorum, helper invocation, raw audit access, and rejection paths record typed `IvPolicyDecision` variants in provenance.

**Rationale**: Policies change returned IV. Typed decisions let tests assert exact candidate sets, rejected inputs, accepted inputs, helper identity, and rejection causes.

**Alternatives considered**:

- Store free-form policy text: rejected because it cannot be exhaustively tested.
- Store only policy IDs: rejected because a policy ID does not prove which candidates or inputs were considered.

## Decision: Aggregate greeks are a typed product

NT aggregate greeks events are preserved raw and indexed into `IvAggregateGreeks` with source, selector, timestamps, values, and provenance.

**Rationale**: Aggregate greeks were part of the requested NT capability set. Leaving them as raw-only would create a hidden second-class path.

**Alternatives considered**:

- Raw-only aggregate greeks access: rejected because strategies would need custom parsing/state.
- Fold aggregate greeks into `IvGreeksPoint`: rejected because aggregate selectors are not necessarily one instrument-level IV point.

## Decision: Derived IV uses NT helpers only with complete configured inputs and helper policy

Derived IV is available when `IvHelperPolicy` selects a ledger-supported NT helper and the request supplies or resolves an `IvDerivedInputSet` under `IvDerivedInputPolicy`: option price, underlying price, strike, option side, time-to-expiry, rate, carry, source timestamps, and accepted convention. Missing helper policy, incompatible helper signature, expired operator-configured values, or invalid inputs reject.

**Rationale**: NT helpers are part of the capability set, but IV derivation is invalid if required assumptions are guessed.

**Alternatives considered**:

- Hardcoded default rate/carry/time convention: rejected by no-hardcodes and correctness.
- Strategy-local derivation: rejected because it bypasses the IV engine.
- Infer helper from product kind: rejected because helper choice can change output semantics and must be provenance-recorded.

## Decision: Projection is explicit

Scalar IV from smiles, surfaces, aggregate products with configured aggregate IV values, or IV evidence requires `IvProjectionPolicy`, including `max_projection_input_skew_ns`.

**Rationale**: Projection can change the answer. It must name the basis, source eligibility, strike/tenor selection, evidence mapping, temporal skew bound, and any interpolation/fallback/quorum dependency.

**Alternatives considered**:

- Return the nearest point by default: rejected because it silently changes strategy behavior.
- Let strategies project after raw or smile queries: rejected because it duplicates IV mechanics and weakens provenance.

## Decision: Interpolation, fallback, extrapolation, and quorum are explicit policy objects

Interpolation, fallback, extrapolation, and quorum behavior are TOML-owned typed policies. Query results record policy decisions in provenance. Missing policy, unknown policy, insufficient inputs, no qualifying fallback candidate, or quorum failure rejects.

**Rationale**: These policies can change the returned IV. They must be auditable and fail closed.

**Alternatives considered**:

- Hardcode default interpolation or fallback: rejected by no-hardcodes and because it changes strategy behavior silently.
- Let strategies apply policy after raw queries: rejected because it duplicates IV mechanics and weakens provenance.

## Decision: Custom implied-volatility data is IV evidence, not an option-chain IV point

Custom implied-volatility data from NT adapters is stored as `IvEvidence`. It can be queried directly and can participate in configured projection/fallback policy, but it is not mislabeled as instrument-level option-chain IV.

**Rationale**: Custom IV evidence can be useful while representing different semantics than per-instrument option IV.

**Alternatives considered**:

- Store custom implied-volatility evidence as `IvPoint`: rejected because it loses semantic distinction.
- Ignore custom implied-volatility data: rejected because it misses NT capability.

## Decision: Raw payload access requires an audit policy

Each IV profile defines `IvAuditPolicy` for enabled raw products, authorized audit handles, access purposes, eligible sources, and audit retention. Strategies are never authorized audit handles.

**Rationale**: Preserving raw NT payloads is required, but raw dereference must remain audit/replay-only and source-fenced away from strategy logic.

**Alternatives considered**:

- Make raw access globally available to any internal caller: rejected because strategies could bypass product provenance.
- Use only source-fence without config policy: rejected because audit access still needs runtime authorization and retention boundaries.

## Decision: Strategy authorization can be profile-wide or selector-scoped

`IvSelectorAuthorization` lets a profile declare profile-wide strategy access or selector-scoped strategy access by product kind, source ID, and selector fingerprint.

**Rationale**: Some strategies should consume every product in a profile, while others need narrower access. The choice must be explicit and testable.

**Alternatives considered**:

- Only profile-wide authorization: rejected because it over-grants when a profile shares sources across strategy families.
- Only selector-scoped authorization: rejected because it creates unnecessary config noise for simple profiles.

## Decision: NT timestamp handling is explicit

NT timestamps are represented as nanosecond timestamps in the IV engine. Any millisecond view for existing Bolt integration must use a named conversion helper with tests.

**Rationale**: Current Bolt modules often use millisecond fields, while NT IV/options data uses nanosecond timestamps. Silent conversion is a correctness risk.

**Alternatives considered**:

- Convert all timestamps to milliseconds on ingest: rejected because it loses precision.
- Mix integer timestamp units by convention: rejected because tests cannot reliably detect misuse.

## Decision: Lifecycle, memory bounds, and retention are first-class

The engine tracks subscription state, unsubscribe, reload, source removal, stale marking, source generation, retention eviction, and per-profile memory bounds. Removed, failed, stale, or old-generation data cannot satisfy current queries.

**Rationale**: Option-chain surfaces can grow quickly and stale data is dangerous if it appears current.

**Alternatives considered**:

- Keep latest value forever: rejected because removed/stale subscriptions would leak into decisions.
- Let each strategy filter staleness: rejected because it duplicates policy.

## Decision: Strategy IV source-fence is global and CI-wired

Strategy modules cannot import NT IV/options subscription APIs, call NT IV/greeks math helpers for strategy-local IV derivation, import raw payload audit readers, import raw IV payload DTOs, request raw-payload product kinds through strategy handles, or derive IV-shaped products from raw IV payload values. The IV source-fence is wired into `just source-fence` through a checked verifier or test target.

**Rationale**: Conditional source-fence based on runtime config is difficult to enforce statically and can recreate dual IV paths.

**Alternatives considered**:

- Reject only for strategies configured to use IV: rejected because static source-fence cannot reliably know runtime profile bindings.
- Allow strategy-local helper derivation for edge cases: rejected because the IV engine is the single owner of derived IV provenance.
- Allow strategy raw payload dereference with a policy rule not to derive IV: rejected because it is not enforceable enough and recreates a bypass path.

## Decision: IV schema versions fail closed

The IV TOML schema version is explicit, and unknown or unsupported versions reject at startup before source planning.

**Rationale**: IV profiles own runtime behavior. Loading an unknown schema version could silently reinterpret selectors, policies, source kinds, or derived-input rules.

**Alternatives considered**:

- Best-effort schema compatibility: rejected because it can silently change strategy-visible IV outputs.

## Decision: This IV packet is the active Speckit planning pointer

For this planning branch, `.specify/feature.json` and the AGENTS Speckit block point to `specs/026-nt-backed-iv-engine/` so `$speckit-plan` and `$speckit-tasks` operate on the IV packet.

**Rationale**: The user explicitly requested a detailed Speckit plan and tasks for the IV engine. The active pointer must match the current planning deliverable.

**Alternatives considered**:

- Leave the pointer on another feature and require explicit paths: rejected because Speckit setup scripts would target the wrong plan.
