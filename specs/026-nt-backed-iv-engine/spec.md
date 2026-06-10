# Feature Specification: NT-Backed IV Engine

**Feature Branch**: `026-nt-backed-iv-engine`
**Created**: 2026-06-07
**Status**: Draft
**Input**: Build a standalone IV engine that uses all IV, option, greeks, option-chain, and derived implied-volatility capabilities exposed by the NautilusTrader Rust APIs pinned in `Cargo.toml`. Strategies must be able to consume the engine outputs through generic APIs. The engine must not be hardcoded to a strategy, venue, market family, asset, cadence, or example instrument.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Inventory NT IV capabilities completely (Priority: P1)

As the maintainer, I need a source-backed inventory of every IV/options capability exposed by the Cargo-pinned NautilusTrader checkout, so the engine scope is defined by NT source evidence rather than by memory, examples, or a handpicked subset.

**Why this priority**: "Use all of what NT offers" is not testable until "all" is tied to a repeatable pinned-source inventory. This inventory is the contract for the rest of the feature.

**Independent Test**: Run the inventory test against the NT checkout resolved from `Cargo.toml`; it fails if any known IV/options surface is missing from the engine capability ledger.

**Acceptance Scenarios**:

1. **Given** the Cargo-pinned NT checkout, **When** the inventory command scans model data, greeks helpers, msgbus subscriptions, data-actor subscriptions, data-engine publications, option-chain manager surfaces, adapter option-greeks support, and custom implied-volatility data types, **Then** the feature records each supported surface in the IV capability ledger.
2. **Given** a newly discovered NT IV/options surface, **When** the ledger does not classify it as supported, intentionally excluded with rationale, or not reachable from the Rust binary, **Then** the inventory test fails.
3. **Given** a stale hardcoded NT revision in a doc, **When** it conflicts with `Cargo.toml`, **Then** `Cargo.toml` remains the only source of truth and the stale doc value is ignored for IV scope.
4. **Given** the locked dependency graph, **When** the ledger test runs, **Then** it resolves the NT checkout through Cargo metadata and `Cargo.lock` evidence rather than a hand-maintained checkout path.
5. **Given** NT adds a public IV/options symbol outside the seed scan families, **When** the whole-checkout candidate sweep sees IV/options terms in its path, symbol, doc comment, or enclosing module, **Then** the ledger test fails until the candidate is classified.

---

### User Story 2 - Subscribe to configured NT IV/options sources (Priority: P1)

As a strategy operator, I need the IV engine to subscribe through NT to every configured IV/options source type NT exposes, so strategies can rely on one engine for option greeks, option-chain slices, aggregate greeks, and custom IV evidence.

**Why this priority**: The engine must not merely normalize offline samples; it must own the live NT subscription boundary for IV/options data.

**Independent Test**: A test data actor records the NT subscribe/unsubscribe calls requested by the IV subscription planner for a TOML fixture where one IV profile owns its sources, strategy access, lifecycle, and query policies.

**Acceptance Scenarios**:

1. **Given** a TOML config with option-greeks sources, **When** the IV engine starts, **Then** it issues NT option-greeks subscriptions for every configured instrument through the configured client and passes configured NT params through unchanged.
2. **Given** a TOML config with option-chain sources, **When** the IV engine starts, **Then** it issues NT option-chain subscriptions for every configured option series and configured strike range.
3. **Given** a TOML config with aggregate greeks sources, **When** the IV engine starts, **Then** it subscribes to NT greeks topics for the configured underlying selectors.
4. **Given** a TOML config with custom implied-volatility data sources exposed by NT adapters, **When** the IV engine starts, **Then** it subscribes through NT custom-data plumbing and records those events as separate IV evidence, not as option-chain IV.
5. **Given** a source removed from TOML, **When** the engine applies a reload plan or stops, **Then** it unsubscribes from the matching NT source and prevents stale data from appearing fresh.
6. **Given** an operator swaps, removes, or renames an IV source inside a profile, **When** TOML is updated, **Then** the source lifecycle, strategy authorization, and query policies are changed in that single profile boundary without editing a separate allow-list section.
7. **Given** a configured source cannot be mapped to an NT runtime subscription operation or its subscription fails, **When** the engine starts or applies a reload plan, **Then** source health records the failure and current queries for that source reject.

---

### User Story 3 - Preserve raw NT data and expose indexed IV products (Priority: P1)

As a strategy author, I need generic indexed IV products backed by preserved raw NT payloads, so strategies can use all IV-relevant NT information without coupling themselves to live subscription mechanics or reimplementing engine state.

**Why this priority**: A normalized scalar would throw away NT value; raw-only strategy pass-through would force every strategy to rebuild IV state. The engine must preserve raw payloads internally and expose strategy-safe products.

**Independent Test**: Feed sample NT `OptionGreeks`, `OptionChainSlice`, aggregate greeks, and custom implied-volatility events into the engine; assert audit raw retrieval returns the original payloads and strategy queries return equivalent IV points, smiles, surfaces, aggregate greeks products, custom IV evidence, provenance, and source-health state without exposing raw payload DTOs to strategy handles.

**Acceptance Scenarios**:

1. **Given** an NT option-greeks event with mark, bid, ask IV, greeks, convention, underlying price, open interest, instrument identity, and timestamps, **When** the engine ingests it, **Then** raw access preserves the NT event and indexed access exposes every IV basis and greek value without dropping convention or timestamp fields.
2. **Given** an NT option-chain slice, **When** the engine ingests it, **Then** raw access preserves the chain slice and indexed access exposes call and put smiles keyed by series, side, strike, source, and event time.
3. **Given** an NT aggregate greeks event, **When** the engine ingests it, **Then** raw access preserves the NT event and indexed access exposes an aggregate greeks product with source, selector, timestamps, and provenance.
4. **Given** a strategy query for a full surface, **When** the store contains multiple series for the same configured surface selector, **Then** the engine returns a surface view with all retained smiles and provenance for each point.
5. **Given** an audit or replay query for raw NT payloads, **When** matching payloads exist within retention bounds, **Then** the engine returns them through an audit/replay handle with provenance and access purpose.
6. **Given** a strategy query handle requests raw NT payloads, **When** the IV API evaluates the request, **Then** the query rejects and returns only product-level APIs to strategy code.

---

### User Story 4 - Derive IV with NT math helpers when configured inputs are complete (Priority: P1)

As a strategy author, I need the IV engine to use NT's implied-volatility and greeks math helpers when configured input data is available, so derived IV is part of the engine rather than a strategy-local calculation.

**Why this priority**: NT exposes IV/greeks math helpers. Excluding them would miss part of the requested NT capability set.

**Independent Test**: Use deterministic inputs with known finite outputs and incomplete-input fixtures; assert derived IV uses NT helpers when complete and fails closed when any required input is absent or invalid.

**Acceptance Scenarios**:

1. **Given** configured option price, underlying price, strike, option side, time-to-expiry, rate, and carry inputs, **When** derived IV is requested, **Then** the engine calls the configured NT math helper and records all inputs, output, and helper identity in provenance.
2. **Given** any missing, non-finite, non-positive where positive is required, or convention-incompatible derived-IV input, **When** derived IV is requested, **Then** the engine rejects the derivation with a typed reason and does not guess defaults.
3. **Given** NT helper output is zero, non-finite, or outside configured IV bounds, **When** the engine validates the output, **Then** the derived IV point is rejected and source health records the failure.
4. **Given** a derived-IV query supplies only some helper inputs, **When** the owning profile's derived-input policy cannot resolve the rest from configured source references, instrument metadata, or operator-configured values, **Then** the query rejects without using defaults.

---

### User Story 5 - Enforce generic config and lifecycle rules (Priority: P1)

As the operator, I need all IV runtime behavior selected by TOML and validated by Rust types, so changing sources, subscriptions, retention, freshness, bases, and query policies never requires code edits and never silently changes behavior through typoed strings.

**Why this priority**: This is the repo's no-hardcode/no-dual-path rule applied to IV. Configurability must not become an untested pricing language.

**Independent Test**: Load valid and invalid TOML fixtures covering every enum, numeric bound, profile boundary, source selector, duplicate source, empty selector, unsupported policy, retention limit, freshness rule, policy conflict, and unit conversion.

**Acceptance Scenarios**:

1. **Given** valid TOML for every supported source type and IV policy, **When** config loads, **Then** it maps to typed Rust config without implicit runtime defaults.
2. **Given** an unknown source kind, IV basis, interpolation policy, fallback policy, subscription selector, or convention rule, **When** config loads, **Then** startup fails closed with a diagnostic pointing to the exact TOML field.
3. **Given** timestamped NT data in nanoseconds, **When** the engine evaluates freshness, retention, and query as-of filters, **Then** every conversion is explicit and tests prove no milliseconds/nanoseconds mixup.
4. **Given** store retention limits, **When** subscriptions churn or sources stop, **Then** old payloads are evicted or marked stale according to TOML and cannot be returned as current.
5. **Given** interpolation, fallback, projection, extrapolation, or quorum policy is configured, **When** a query needs that policy, **Then** the engine applies only the configured typed policy and records every source, basis, convention, interpolation axis, fallback candidate, and quorum decision in provenance.

---

### User Story 6 - Let strategies consume IV generically (Priority: P1)

As a strategy author, I need a strategy-agnostic IV API that exposes all strategy-safe engine products, so any strategy can ask for IV points, greeks, smiles, surfaces, custom IV evidence, aggregate greeks, projections, source health, and derived IV without strategy-specific code paths.

**Why this priority**: The user explicitly requires strategies to use the IV engine, while keeping the engine free of strategy, venue, market, asset, and cadence hardcodes.

**Independent Test**: Create two test strategy harnesses with different configured selectors; both consume the same IV engine API and neither imports subscription or ingestion internals.

**Acceptance Scenarios**:

1. **Given** any registered strategy with an IV selector in TOML, **When** it requests IV data, **Then** it uses the same generic IV API as every other strategy.
2. **Given** a strategy tries to bypass the IV engine by subscribing directly to NT IV/options topics inside strategy code, **When** source-fence checks run, **Then** the check fails regardless of which strategies are currently configured to use IV.
3. **Given** a strategy requests data for a selector that is not configured for that strategy, **When** the IV API evaluates the request, **Then** it rejects the request without leaking another strategy's source data.
4. **Given** a strategy needs IV-shaped values from raw NT evidence, **When** it queries the IV engine, **Then** it must request an IV engine product, projection, or derived product rather than receiving raw payload DTOs or deriving IV locally.

### Edge Cases

- NT exposes a new IV/options surface at the Cargo-pinned checkout after dependency refresh; the inventory gate fails until the IV capability ledger classifies it.
- A configured source emits option greeks without any IV basis; raw data is preserved, indexed IV point creation is rejected, and source health records the missing basis.
- A chain slice contains quotes for strikes with no nested greeks; raw chain is preserved, smile IV points are created only for strikes with usable IV evidence.
- Two configured sources publish the same instrument and basis at the same event time; the store preserves both and query policy determines ordering without dropping provenance.
- A source clock is ahead of query as-of time; freshness evaluation rejects current views until policy permits the timestamp.
- A derived-IV request has price and underlying inputs from different source timestamps outside configured skew; derivation rejects rather than blending silently.
- A custom implied-volatility data event represents index-style IV evidence rather than instrument-level IV; it is stored as IV evidence and never mislabelled as an option-chain point.
- A strategy requests scalar IV when only a smile is available; the request must choose a configured projection policy or reject.
- A projection combines smiles, surfaces, aggregate products with configured aggregate IV values, or IV evidence with event times outside configured projection skew; the query rejects.
- A strategy requests interpolation outside the configured smile or surface domain; the query rejects unless TOML explicitly permits that extrapolation mode.
- A query requires quorum across sources and the configured quorum threshold is not met; the query rejects instead of falling back to a single source.
- A source ID is renamed inside an IV profile; no other TOML section is edited to preserve strategy authorization for that profile.
- A subscription reload races with an older event; the event's subscription generation prevents stale-generation data from satisfying current queries.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST generate and maintain a source-backed IV capability ledger from the NautilusTrader checkout pinned by `Cargo.toml`.
- **FR-002**: System MUST classify every NT IV/options capability found by the ledger as supported by this engine, unreachable from the Rust binary, or explicitly out of scope with user-approved rationale.
- **FR-003**: System MUST support NT option-greeks subscriptions through configured data clients and configured instrument selectors.
- **FR-004**: System MUST support NT option-chain subscriptions through configured data clients, configured option-series selectors, and configured strike-range policies.
- **FR-005**: System MUST support NT aggregate greeks subscriptions where NT exposes them and MUST model aggregate greeks as a queryable product rather than leaving them as an untyped raw-only side path.
- **FR-006**: System MUST support NT custom implied-volatility data sources where NT adapters expose them through the Rust runtime.
- **FR-007**: System MUST preserve raw NT `OptionGreeks` payloads, including instrument identity, greeks convention, all `OptionGreekValues`, mark IV, bid IV, ask IV, underlying price, open interest, event timestamp, and init timestamp.
- **FR-008**: System MUST preserve raw NT `OptionChainSlice` payloads, including series identity, ATM strike, call and put strike maps, nested quotes, nested greeks, and timestamps.
- **FR-009**: System MUST index IV points for every usable mark, bid, and ask IV value without collapsing them into one basis.
- **FR-010**: System MUST index greeks, convention, underlying price, open interest, source identity, and provenance alongside each IV point.
- **FR-011**: System MUST build smile views from option-chain slices and retained per-instrument greeks without requiring asset, venue, market, cadence, or strategy-specific logic.
- **FR-012**: System MUST build surface views from retained smiles across configured series selectors.
- **FR-013**: System MUST expose raw NT payload queries through an audit/replay API and expose aggregate greeks queries, custom IV evidence queries, and indexed IV-product queries through one strategy-agnostic product API.
- **FR-014**: System MUST allow strategies to consume IV engine product outputs, including IV points, smiles, surfaces, source health, custom IV evidence, aggregate greeks, projections, and derived-IV products.
- **FR-015**: System MUST prevent strategy code from owning IV subscription mechanics or strategy-local NT helper-backed IV derivation; the IV engine is the only live runtime owner of IV/options subscriptions and IV derivation.
- **FR-016**: System MUST use NT math helpers for derived IV and derived greeks when configured inputs are complete and valid.
- **FR-017**: System MUST reject derived-IV requests when any required input, convention, timestamp, side, rate, carry, price, strike, or time-to-expiry field is missing or invalid.
- **FR-018**: System MUST record complete provenance for every raw, indexed, derived, projected, policy-produced, and rejected IV output.
- **FR-019**: System MUST model custom implied-volatility data as a separate IV evidence product, not as an option-chain IV point.
- **FR-020**: System MUST make IV profiles, source selection, freshness, retention, allowed bases, accepted conventions, interpolation policy, fallback policy, extrapolation policy, quorum policy, and query projection policy TOML-owned and Rust-validated.
- **FR-021**: System MUST reject unknown TOML policies, unknown source kinds, empty source selectors, duplicate source IDs, invalid numeric bounds, unit-ambiguous fields, and unknown or unsupported IV TOML schema versions at startup.
- **FR-022**: System MUST preserve NT timestamps in nanoseconds internally or convert through a named type with tests proving the conversion.
- **FR-023**: System MUST mark stale data stale and prevent stale data from satisfying current queries unless the strategy explicitly requests historical data.
- **FR-024**: System MUST implement subscription lifecycle planning and runtime-state handling for start, stop, reload, unsubscribe, and source removal. This feature does not add a production config hot-reload trigger to the live-node runner.
- **FR-025**: System MUST enforce retention bounds for raw payloads, indexed points, smiles, surfaces, derived products, and source-health events.
- **FR-026**: System MUST expose typed reject reasons for missing IV basis, invalid IV value, unsupported convention, stale data, clock skew, incomplete derived inputs, helper failure, source not configured, selector not authorized, retention miss, interpolation rejection, extrapolation rejection, fallback rejection, and quorum not met.
- **FR-027**: System MUST avoid any hardcoded strategy name, venue name, market family, asset identifier, cadence, instrument ID, source ID, quantity, timeout, or policy value in runtime logic.
- **FR-028**: System MUST include source-fence tests proving the IV engine core does not import concrete strategy modules.
- **FR-029**: System MUST include source-fence tests proving strategy modules do not subscribe directly to NT IV/options topics or call NT IV math helpers for strategy-local IV derivation.
- **FR-030**: System MUST document that FV/RV engines are not prerequisites for the IV engine and are not part of this feature's deliverable.
- **FR-031**: System MUST integrate the IV engine into the existing Rust crate exports, root TOML loading, live-node startup path, and strategy registration/query-handle path so the deliverable is a live NT-backed engine rather than an isolated library.
- **FR-032**: System MUST group IV source lifecycle, source policies, strategy authorization, enabled products, and query policies inside one TOML-owned IV profile so changing a source does not require edits in multiple config sections.
- **FR-033**: System MUST define interpolation policy with explicit axes, method, minimum input points, source eligibility, and extrapolation behavior.
- **FR-034**: System MUST define fallback policy with explicit ordered candidates, source eligibility, maximum timestamp skew, provenance, and rejection behavior when no candidate qualifies.
- **FR-035**: System MUST define quorum policy with explicit source count, source eligibility, agreement band, tie-breaking, and rejection behavior when quorum is not met.
- **FR-036**: System MUST define `IvSelector` as a typed Rust-validated union for option-greeks, option-chain, aggregate-greeks, custom-implied-volatility, smile, surface, and IV-evidence selectors.
- **FR-037**: System MUST reject source configs and queries whose source-scope or query-scope selector variant does not match the configured source kind or requested product kind.
- **FR-038**: System MUST define `IvProvenance` and attach it to every raw event, indexed product, derived product, projection, policy output, and rejection.
- **FR-039**: System MUST define `IvProjectionPolicy` for scalar projection from smiles, surfaces, aggregate products with configured aggregate IV values, or custom IV evidence and MUST reject scalar requests when the policy is absent, invalid, input aggregate products lack aggregate IV values, or input timestamps exceed configured projection skew.
- **FR-040**: System MUST define `IvDerivedInputPolicy` and `IvDerivedInputSet` so derived IV queries either supply or profile-resolve every NT helper input with provenance.
- **FR-041**: System MUST bind live sources through NT runtime subscription APIs and event handlers, with source-health transitions for configured, subscribing, active, stale, unsubscribing, removed, subscription-failed, and rejected states.
- **FR-042**: System MUST generate the capability ledger from Cargo metadata and `Cargo.lock` evidence for the pinned NT checkout; handwritten NT revisions or local checkout paths are not accepted.
- **FR-043**: System MUST wire IV source-fence enforcement into the repository `just source-fence` path or an invoked checked test so bypasses fail in CI.
- **FR-044**: System MUST make raw payload access an engine-mediated audit/replay evidence path, not a strategy-facing payload-dereference path or a permission for strategies to build strategy-local IV-shaped products.
- **FR-045**: System MUST keep per-profile memory bounds for raw events, indexed points, smiles, surfaces, derived products, and source-health events TOML-owned and enforced.
- **FR-046**: System MUST source-fence `src/strategies/**` from importing raw IV payload audit readers, raw IV payload DTOs, and raw-payload strategy query product kinds.
- **FR-047**: System MUST make the NT capability ledger perform a whole-checkout IV/options candidate sweep in addition to seed-family scans.
- **FR-048**: System MUST define `IvHelperPolicy` so NT math helper selection, helper parameter signatures, output validation, and helper provenance are TOML-owned or ledger-owned rather than inferred in derivation code.
- **FR-049**: System MUST define typed `IvPolicyDecision` variants for projection, interpolation, extrapolation, fallback, quorum, helper invocation, and rejection decisions; free-form policy decision strings are not accepted.
- **FR-050**: System MUST define `IvAuditPolicy` inside each `IvProfile` so raw payload access has explicit enabled raw products, access purposes, authorized audit handles, retention limits, and source eligibility.
- **FR-051**: System MUST define `IvSelectorAuthorization` so each profile can choose whether strategy access is profile-wide or selector-scoped; selector-scoped profiles must map strategy IDs to allowed products and selector fingerprints.
- **FR-052**: System MUST define `IvNumericBounds` and `IvConventionBounds` for input and output validation, including finiteness, positivity, IV ranges, rate/carry ranges, time-to-expiry ranges, strike ranges, and convention eligibility.
- **FR-053**: System MUST define the accepted IV TOML schema-version set, version bump rule, and migration behavior; unknown versions reject before subscription planning.
- **FR-054**: System MUST include option-microstructure terms such as strike, expiry, expiration, tenor, moneyness, skew, premium, and vol in the whole-checkout candidate sweep so NT symbols without explicit `iv` or `implied` names are still surfaced for classification.

### Key Entities *(include if feature involves data)*

- **IvCapabilityLedger**: Source-backed inventory of NT IV/options APIs at the Cargo-pinned revision.
- **IvProfile**: TOML lifecycle boundary that owns IV sources, source policies, strategy authorization, enabled products, retention, freshness, interpolation, fallback, quorum, projection, and derived-input policy.
- **IvAuditPolicy**: Per-profile audit/replay boundary for raw payload access, authorized audit handles, allowed raw products, and audit retention.
- **IvSourceConfig**: TOML-derived source definition containing source ID, data client, source kind, selectors, params, retention, freshness, accepted conventions, and enabled products.
- **IvSelector**: Typed union for source and query selectors, validated against source kind and product kind.
- **IvSelectorAuthorization**: Strategy-to-product and strategy-to-selector authorization rule for profile-wide or selector-scoped IV access.
- **IvSubscriptionPlan**: Concrete NT subscribe/unsubscribe operations derived from `IvSourceConfig`.
- **IvRuntimeBinding**: Live NT data actor, msgbus, subscription, event-router, and source-health binding for one configured source.
- **IvRawEvent**: Preserved NT payload or custom implied-volatility event with source identity and receive metadata.
- **IvRawAuditAccess**: Audit, replay, and test-only raw payload reader that is not available through strategy query handles.
- **IvProvenance**: Required audit schema for raw, indexed, derived, projected, policy, and rejected outputs.
- **IvPolicyDecision**: Typed decision record for projection, interpolation, extrapolation, fallback, quorum, helper, and rejection paths.
- **IvPoint**: One timestamped IV value for one instrument, basis, source, convention, and provenance record.
- **IvGreeksPoint**: IV point plus all NT greek values and related fields.
- **IvAggregateGreeks**: Queryable aggregate greeks product derived from NT aggregate greeks events with selector, source, timestamps, values, and provenance.
- **IvSmile**: Strike-indexed IV points for one option series, side, source, and as-of time.
- **IvSurface**: Collection of smiles across configured series selectors.
- **IvEvidence**: Custom IV evidence that is not equivalent to instrument-level option IV.
- **IvHelperPolicy**: TOML and ledger-backed policy for choosing NT IV/greeks helpers, allowed parameter signatures, output bounds, and helper provenance.
- **IvDerivedInputPolicy**: Typed policy for resolving option price, underlying, strike, side, time-to-expiry, rate, carry, timestamps, and convention.
- **IvDerivedInputSet**: Resolved helper input bundle with provenance for one derived-IV request.
- **IvDerivedPoint**: IV and greeks produced by NT math helpers with complete input provenance.
- **IvProjectionPolicy**: Typed policy for scalar projection from smile, surface, aggregate-IV, or evidence products.
- **IvInterpolationPolicy**: Typed policy for smile/surface interpolation and extrapolation rejection or permission.
- **IvFallbackPolicy**: Typed policy for ordered source/product/basis fallback.
- **IvQuorumPolicy**: Typed policy for multi-source agreement before returning a value.
- **IvNumericBounds**: Typed numeric and convention validation boundaries for input, output, projection, and helper data.
- **IvMemoryBounds**: TOML-owned retention and memory limits for IV stores and health history.
- **IvSourceHealth**: Subscription state, freshness, last event, last rejection, rejection counts, and retention state for one source.
- **IvQuery**: Strategy-facing request for points, greeks, smiles, surfaces, custom IV evidence, aggregate greeks, projections, source health, or derived IV.
- **IvRejectReason**: Typed fail-closed reason for rejected ingestion, derivation, or query.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The capability ledger test fails if any NT IV/options surface found at the Cargo-pinned checkout is unclassified.
- **SC-002**: A single TOML fixture can configure every supported NT IV/options source kind without code changes.
- **SC-003**: Ingestion tests prove mark, bid, ask IV, full greeks, convention, underlying price, open interest, timestamps, and raw payloads are preserved for option-greeks events.
- **SC-004**: Chain tests prove call and put smiles and surface views are built from option-chain slices without asset, venue, market, strategy, or cadence constants.
- **SC-005**: Derived-IV tests prove NT helper calls succeed with complete inputs and reject with typed reasons for every missing or invalid input class.
- **SC-006**: Strategy harness tests prove at least two strategy instances can query the same IV engine API with different selectors and no direct NT IV subscription code.
- **SC-007**: Source-fence checks prove no runtime hardcoded strategy, venue, market, asset, instrument, cadence, source ID, timeout, or policy value exists in IV core logic.
- **SC-008**: Store lifecycle tests prove unsubscribe, source removal, stale marking, and retention eviction prevent removed or stale data from appearing current.
- **SC-009**: Config tests prove swapping or renaming a source inside one IV profile requires no edits outside that profile.
- **SC-010**: Live integration tests prove root TOML loading, live-node startup, NT subscription planning, and strategy query-handle registration all use the IV engine path.
- **SC-011**: Aggregate greeks tests prove NT aggregate greeks events are preserved raw and exposed as typed aggregate products.
- **SC-012**: Policy tests prove interpolation, extrapolation, fallback, and quorum decisions are TOML-owned, provenance-recorded, and fail closed when policy requirements are not met.
- **SC-013**: Selector tests prove every source kind and product kind accepts only its typed selector variant and rejects mismatches.
- **SC-014**: Provenance tests prove every raw, indexed, derived, projected, policy, and rejected output includes required provenance fields.
- **SC-015**: Ledger tests prove the NT checkout is resolved from Cargo metadata and `Cargo.lock`, and fail when a discovered IV/options surface is unclassified.
- **SC-016**: Source-fence tests run through `just source-fence` and reject strategy-local NT IV subscriptions, NT helper-backed derivation, raw IV payload audit reader imports, raw IV payload DTO imports, raw-payload product requests through strategy handles, and IV-shaped derivation from raw payload values.
- **SC-017**: Derived-input and projection tests prove missing policies, missing inputs, unresolved rate/carry/time fields, scalar projection without explicit policy, and projection input skew violations all reject.
- **SC-018**: Capability-sweep tests prove an IV/options-like public NT symbol placed outside the seed families is surfaced as an unclassified candidate until classified.
- **SC-019**: Helper-policy tests prove NT helper selection is deterministic, provenance-recorded, and rejected when helper policy, helper signature, or output bounds are missing or invalid.
- **SC-020**: Provenance tests prove every policy-produced output includes the typed `IvPolicyDecision` variant required for the applied policy rather than a free-form string.
- **SC-021**: Audit-policy tests prove raw payload access is available only through configured audit/replay handles and is never injected into strategy query handles.
- **SC-022**: Selector-authorization tests prove profile-wide access and selector-scoped access both enforce configured products and selector fingerprints exactly.
- **SC-023**: Bound-schema tests prove every numeric and convention bound rejects non-finite, out-of-range, unit-ambiguous, or convention-ineligible input before ingestion, projection, or helper output is accepted.
- **SC-024**: Schema-version tests prove accepted IV TOML schema versions load, unknown versions reject before source planning, and version-bump migrations are explicit.

## Assumptions

- `Cargo.toml` is the only source of truth for the NautilusTrader revision.
- "All NT offers" means every IV/options capability reachable from the Cargo-pinned NT Rust APIs used by the pure Rust binary, not Python-only helpers or unreachable development examples.
- Strategies may consume IV engine product outputs, while raw NT payloads remain engine-owned and audit/replay-only.
- Runtime behavior is configured in TOML; Rust owns validation, typed policy enums, and fail-closed execution.
- This feature does not implement FV or RV engines and does not depend on either one.
