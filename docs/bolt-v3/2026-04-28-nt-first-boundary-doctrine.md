# Bolt-v3 NT-First Boundary Doctrine

Status: approved doctrine

Path: `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md`
Last full NT doctrine audit rev: `56a438216442f079edf322a39cdc0d9e655ba6d8`
Last full NT doctrine audit date: 2026-04-28
Last NT pin compatibility verified rev: `38b912a8b0fe14e4046773973ff46a3b798b1e3e`
Last NT pin compatibility verified date: 2026-04-30
Owner: Bolt-v3 maintainers

This artifact records the current Bolt-v3 boundary doctrine for
NautilusTrader-owned provider behavior. It is a reviewer and future-session
artifact, not an implementation plan.

Status lifecycle: `candidate` -> `approved` -> `superseded` -> `archived`.
Candidate becomes approved only through explicit review. Approved becomes
superseded when a newer doctrine artifact replaces it. Superseded artifacts are
archived when no active slice may cite them.

## Governance

Approved decisions can change only through an explicit review packet that names
the decision ID being changed. Open decisions cannot be implemented as doctrine
until promoted by review. Residuals remain accepted violations only while they
carry lifecycle metadata and a retirement condition.

If a residual carries no retirement-progress evidence for two consecutive
implementation slices, it must be explicitly re-reviewed: promote it to an open
decision, accept it as a permanent exception with justification, assign a
retirement slice, or revise its retirement condition.

Retirement-progress evidence means the residual is named in a slice's doctrine
statement as retired, partially addressed, or intentionally left with
justification.

`BOLT-POLICY[scope]` rules follow the same retirement-progress discipline as
residuals. If a scope rule's removal condition is not met for two consecutive
implementation slices, it must be re-reviewed: promote it to permanent
`BOLT-POLICY`, demote it to a residual, remove it, or revise its removal
condition.

If the NautilusTrader rev in `Cargo.toml` differs from `Last full NT doctrine
audit rev`, NT-evidence-backed decisions in this file are stale for any claim
that depends on unaudited upstream behavior. A pin compatibility slice may
update `Last NT pin compatibility verified rev`, but that proves only the
declared compatibility surface for that slice. It does not refresh the full
doctrine audit.

Verifier rules are code artifacts. Each verifier listed here must eventually
record its physical enforcement location, such as a Cargo test name, source
scanner, or CI script. Until then it is process-only.

Verifier table entries follow the same change discipline as approved decisions.
Adding, removing, weakening, or changing the technique or location of a `V-ID`
requires a review packet that cites the verifier ID.

Active documentation authority is intentionally narrow:

- This doctrine is the authority for Bolt-v3 architecture boundaries.
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` is the authority
  for source-backed implementation status.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` and
  `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` are detailed contracts that must
  be reconciled to this doctrine before implementation relies on them.
- `docs/bolt-v3/research/` files are evidence, not policy.
- `docs/bolt-v3/archive/` files are reference only.
- Prior AI responses, review transcripts, and temporary audit packets are not
  authoritative. Only source-backed conclusions folded into this doctrine or
  the status map may guide implementation.

## Relation To Repo Rules

This doctrine is the Bolt-v3-specific interpretation of the repo rules in
`AGENTS.md`. If this file conflicts with `AGENTS.md`, `AGENTS.md` wins.

Decision to repo-rule mapping:

| Doctrine area | Repo rule served |
| --- | --- |
| NT-first Rust boundary | NO HARDCODES, PURE RUST BINARY |
| Provider dispatch by owned bindings | GROUP BY CHANGE |
| SSM-only provider policy | SSM IS THE SINGLE SECRET SOURCE |
| No NT defaults in mapping | NO HARDCODES |
| Explicit residual lifecycle | NO DEBTS |
| One durable doctrine artifact | NO DUAL PATHS |

## Glossary

- NautilusTrader, or NT: the pinned Rust dependency set under the rev recorded
  in this file.
- Bolt TOML policy gate: Bolt's startup-time deserialization and policy layer
  before constructing NT config structs.
- Policy-narrowed mirror: a Bolt TOML struct that intentionally accepts less
  than the matching NT config struct because Bolt forbids NT defaults, env
  fallback, or unsupported scope.
- Binding table: a static list, in a vertical root, of provider-contributed
  validation entries that the root iterates instead of matching on a closed
  enum. It is explicit static registration, not dynamic discovery.
- Provider binding: a provider-owned module boundary that owns the provider key,
  provider TOML sub-shape, provider secret projection, NT config conversion,
  NT factory registration metadata, and provider-specific readiness or
  discovery glue when NT does not already own it.
- Market family: a venue-independent semantic family, such as a binary
  up/down market. A venue may expose markets in that family, but the family is
  not the venue.
- Strategy archetype: a reusable strategy module instantiated by many strategy
  TOML files. An archetype owns its validation and construction contract; core
  runtime assembly must not grow a new branch per strategy instance.
- Vertical root: a `mod.rs` whose direct job is dispatching among concrete
  children under one discriminator, without owning provider, family, or
  archetype policy.
- Residual: a known violation of the target doctrine that is accepted only
  because it is named, reviewed, and has a retirement condition.
- Runtime-bearing field: any NT config field whose value affects runtime
  behavior, including URLs, credentials, account or trader IDs, timeouts,
  retries, limits, polling intervals, subscription flags, provider keys,
  venue keys, filters, feature flags, booleans that alter subscriptions, or
  order and risk quantities.

## Verified Evidence

The following source anchors were verified before writing this doctrine:

- `Cargo.toml` pins NT to release `v1.226.0`
  (`38b912a8b0fe14e4046773973ff46a3b798b1e3e`). The 2026-04-30 pin slice
  verified compile/test compatibility for that release; it did not re-audit all
  NT-owned behaviors cited by this doctrine.
- The NT pin-change audit and compatibility probe are recorded under
  `docs/bolt-v3/research/nt-pin-change/`; the CLOB V2 live-readiness gate
  remains open until live signing, order, fill, collateral, and fee behavior are
  verified.
- NT Rust factories require typed `ClientConfig` structs and downcast them.
  They do not accept raw TOML or `serde::Value`.
- `src/bolt_v3_config.rs` defines closed `VenueKind` and
  `StrategyArchetype` enums.
- `src/bolt_v3_providers/mod.rs` currently dispatches provider validation by
  matching `VenueKind`.
- `src/bolt_v3_archetypes/mod.rs` currently dispatches archetype validation by
  matching `StrategyArchetype`.
- `src/bolt_v3_market_families/mod.rs` currently dispatches family validation
  by matching `RotatingMarketFamily`.
- `src/bolt_v3_validate.rs` currently has a one-venue-per-kind scope rule that
  groups venues by `venue.kind.as_str()`.
- `src/bolt_v3_market_identity.rs` is currently a provider-neutral,
  family-agnostic market-identity boundary with source-level guard tests.
- `src/bolt_v3_adapters.rs` owns `BoltV3VenueAdapterConfig`,
  `MarketSlugFilter` construction, adapter dispatch, and secrets-enum
  extraction.
- `src/bolt_v3_secrets.rs` owns `ResolvedBoltV3VenueSecrets` and closed
  provider secret dispatch.
- `src/bolt_v3_client_registration.rs` owns `BoltV3RegisteredVenue` and closed
  adapter-config consumption.
- `src/bolt_v3_live_node.rs` owns provider-shaped
  `NT_CREDENTIAL_LOG_MODULES`.
- NT Binance config at the pinned rev derives `Debug` over credential-bearing
  fields. NT Polymarket config has manual `Debug`.
- NT Polymarket credential resolution can fall back to `POLYMARKET_FUNDER` when
  `funder` is absent or empty.

## Approved Decisions

### D1. NT-First Boundary

Status: approved

Bolt remains a thin Rust layer over NT. Bolt owns only what NT does not own on
the Rust factory path:

- TOML envelope and schema parsing.
- SSM-only secret policy.
- Explicit runtime values instead of NT defaults or env fallback.
- Safe type conversions into NT config structs.
- Current-scope restrictions.
- Startup checks only when NT would otherwise fail late, fail cryptically, or
  silently use behavior Bolt forbids.

NT owns runtime adapter behavior, protocols, market data, execution, and
constructed-client behavior. Bolt owns both the TOML policy gate and the typed
conversion membrane that produces NT `ClientConfig` structs. Bolt validation is
not a parallel provider framework.

### D2. Provider Validation Dispatch Direction

Status: approved direction, open mechanism

Closed `match VenueKind` dispatch in `src/bolt_v3_providers/mod.rs` must be
replaced by open iteration over provider-contributed validation bindings.
Concrete provider modules own provider key literals.

Open mechanism details:

- Binding struct shape.
- Trait object versus function pointer.
- Unknown-provider behavior after provider identity is no longer a closed enum.
- Iteration semantics.
- Physical verifier shape.

This decision is approved only for provider validation dispatch. Applying the
same pattern to archetypes, market families, or future vertical roots remains
open until separately reviewed.

The binding table is explicit and static by design. Its value is grouping
provider changes into provider-owned modules and one visible registration list,
not dynamic plugin discovery.

The binding struct must not name `VenueKind` as its key type. Provider binding
keys are provider-owned string literals, not closed core enum values.

While `VenueKind` remains closed, this is transitional binding-table dispatch,
not truly open provider identity. The root module still declares concrete child
modules and the registration list; that structural coupling is accepted for
static Rust without proc-macro registration.

### D3. Provider Validation Rule Categories

Status: approved

Every Bolt provider runtime validation rule must fit exactly one of these
runtime categories:

- `BOLT-POLICY`: Bolt rejects behavior NT permits because of deployment,
  operations, security, or runtime-value policy.
- `BOLT-POLICY[scope]`: Bolt rejects behavior NT permits because current
  Bolt-v3 scope does not support it yet. The rule must name a removal or
  revisit condition tied to an open decision or residual.
- `CONVERSION`: the rule is required for safe Rust type conversion into NT
  config structs.
- `FAIL-FAST`: the rule is allowed only if NT would otherwise silently
  misbehave or produce a late or cryptic error without a clear TOML path. The
  rule must cite the pinned NT file and line or symbol that shows the failure
  mode.
- `DRIFT-GUARD`: verifier or process work that detects NT schema or config
  drift. It is not a standalone runtime provider-validation category and must
  not justify reimplementing NT runtime validation. Runtime validation that
  also guards drift must still qualify as `BOLT-POLICY`,
  `BOLT-POLICY[scope]`, `CONVERSION`, or `FAIL-FAST`.

If NT would produce a clear, operator-actionable error at the same startup
stage, Bolt must not duplicate the check as `FAIL-FAST`.
`FAIL-FAST` justification must cite NT failure-mode evidence and explicitly
assert that no equivalent operator-actionable error fires at or before the same
startup stage.

### D4. Polymarket-Updown Glue

Status: approved concrete case

Polymarket-Updown glue may live under the Polymarket provider because the
output is provider-specific NT filter or config construction. It must be a
conversion membrane:

- Family-neutral facts or plans in.
- Provider-specific NT values out.
- No adapter-layer imports just to reuse types. Shared types must move to a
  neutral location first; see O4.
- No family policy, strategy policy, risk policy, secrets logic, defaults, or
  multi-provider logic.

This approves only the Polymarket-Updown case. General cross-vertical and
multi-provider glue ownership remains open.

### D5. Validation, Secrets, And Mapping Coupling

Status: approved principle, open enforcement mechanism

Validation, secret resolution, and adapter mapping must agree. If validation
requires a TOML field, mapping must explicitly pass it to NT and must not
reintroduce NT defaults.

NT config construction must avoid default laundering:

- No `..Default::default()` for NT config structs.
- No builder defaults for runtime-bearing values.
- No derived `From` or `Into` mapping that hides NT defaults.
- No raw maps, flattened maps, catch-all TOML payloads, or unvalidated dynamic
  values cross the Bolt-to-NT membrane unparsed.
- Bolt provider config structs must not use `Option<T>` or `#[serde(default)]`
  for runtime-bearing fields unless Bolt policy explicitly makes the field
  optional. Every runtime-bearing field is required in TOML unless an approved
  rule says otherwise.

The physical enforcement mechanism is open. A field-decision registry, source
scanner, AST-aware lint, or typed test fixture may be selected later. Until
then, this is a process and review rule.

### D6. Future Slice Gate

Status: approved principle, open enforcement mechanism

Every future implementation slice must state:

- Which approved doctrine decision it exercises.
- Which residual it retires or leaves in place.
- Which open decisions it does not settle.
- Which verifier enforces it.
- Why the slice is narrow enough to review.

The enforcement mechanism for this gate is open. Candidate mechanisms include a
slice doc template, PR template, CI check, or verifier test.

## Candidate Direction For Next Slice

Status: candidate clarification for provider-boundary implementation; not an
approved decision until reviewed.

The intended architecture is static Rust registration with dynamic configured
instances. Adding a provider, market family, or strategy archetype may require
adding one owned binding module plus one explicit registration entry. It must
not require editing core matches, closed provider enum variants, closed secret
variants, or core client-registration branches.

Provider bindings are conversion membranes into NT, not Bolt-owned venue
frameworks. A provider binding may own:

- Provider-specific TOML sub-shape.
- Provider secret-path validation and resolved-secret projection.
- Explicit NT config construction for that provider.
- NT data and execution factory registration metadata.
- Provider-specific credential-log metadata.
- Provider-specific discovery, readiness, filter, or cost-fact acquisition only
  where NT does not already expose a suitable Rust fact.

Market families are separate from providers. Discovery may observe and classify
broad provider market inventory, but first-live trading may use only explicitly
supported provider + market-family + strategy-archetype bindings. Unknown,
ambiguous, or unsupported markets are observable evidence, not automatic
trading targets.

Strategy archetypes are reusable modules. Many strategy TOML files may
instantiate the same archetype with different configured targets, venues,
reference inputs, sizing limits, and thresholds. Adding a strategy instance must
not require runtime assembly changes.

Cost and fee facts are infrastructure inputs, but not strategy-owned fee
calculation. Provider or family bindings may prepare cost facts outside the
latency-critical decision path, with freshness and source evidence checked at
startup and before activation. The hot path reads an already-prepared fact
snapshot. Research owns alpha and net-opportunity design; infra owns making the
required facts available without blocking trading decisions on live API calls.

NT Portfolio remains the source of truth for account, balance, position, order,
fill, average-price, and exposure state. Bolt allocation state may exist only as
decision-local evidence, TOML limit application, or audit metadata. It must not
replace NT Portfolio truth.

## Open Decisions

| ID | Open decision | Notes |
| --- | --- | --- |
| O1 | Exact provider binding table shape | Includes struct fields, trait versus function pointer, unknown-provider behavior, and iteration semantics. |
| O2 | Binding pattern for archetypes and market families | Closed dispatch remains residual until separately reviewed. |
| O3 | General cross-vertical and multi-provider glue ownership | Polymarket-Updown does not decide multi-provider or multi-family cases. |
| O4 | Shared cross-vertical type location | Concrete current case: `BoltV3UpdownNowFn` lives in `src/bolt_v3_adapters.rs`, but provider-owned glue must not import adapter modules just to reuse types. |
| O5 | Validation-rule classification storage | Source comments, registry, tests, and doc storage are all undecided. |
| O6 | Field-decision registry and mapping verifier | Must eventually prove validation, secrets, and mapping agreement. |
| O7 | NT rev drift enforcement mechanism | Metadata exists now; CI or test enforcement is undecided. |
| O8 | Optional NT credential fallback policy | Current case: Polymarket `funder=None` can trigger `POLYMARKET_FUNDER` fallback. Closeable in an audit slice that cites NT's `Some(value)` versus env-var precedence. |
| O9 | Future slice gate enforcement | Principle is approved; physical mechanism is open. |
| O10 | Deep immutability between validation and mapping | Need a mechanism to prove validated config is the config mapped into NT. See R15 for the current concrete gap. |
| O11 | Rule relaxation audit process | Any change from rejecting a value to accepting it must re-audit NT runtime couplings at the pinned rev. Closes through a process artifact, not code alone. |
| O12 | Provider binding physical shape beyond validation | Must decide how adapter mapping, secret projection, client registration, credential-log metadata, readiness, discovery, and cost-fact acquisition compose without recreating a venue framework. |
| O13 | Cost and fee fact source contract | Must decide the typed fact model, freshness gates, provenance fields, hot-path snapshot shape, and NT-owned versus Bolt-owned source boundary. |
| O14 | Discovery and classification boundary | Must decide where broad market discovery lives, how classification results are represented, and how unsupported/ambiguous markets are prevented from becoming trading targets. |
| O15 | Strategy archetype construction binding | Must decide the construction interface that lets many strategy TOMLs instantiate one archetype without adding core runtime branches. |
| O16 | Portfolio allocation interface | Must decide the minimum Bolt-side allocation evidence needed for sizing and audit while keeping NT Portfolio authoritative. |

## Named Residuals

Each residual is opened on 2026-04-28 unless a later artifact records a
different date.

| ID | Residual | Evidence anchor | Owner | Retirement condition | Verifier |
| --- | --- | --- | --- | --- | --- |
| R1 | Closed `VenueKind` enum and core-owned provider identity. Subsumes core-owned provider string projection via `as_str()` and serde provider-key literals on closed enum variants. D2 resolves the closed-match symptom, but still depends on `venue.kind.as_str()` until this retires. | `src/bolt_v3_config.rs` | Bolt-v3 maintainers | Core config contains no provider-named enum variants, no `as_str()` provider projection, and no serde rename attribute mapping a variant to a provider key. Backward-compatible TOML string values may remain as documented TOML schema. | Process-only |
| R2 | Closed provider validation dispatch | `src/bolt_v3_providers/mod.rs` | Bolt-v3 maintainers | Provider validation uses provider-owned bindings and no `VenueKind` match in root | Planned verifier |
| R3 | Closed adapter dispatch and adapter config output type | `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Adapter mapping does not produce provider-variant enum requiring downstream closed matching | Process-only |
| R4 | Closed `BoltV3VenueAdapterConfig` construction | `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Mapper no longer constructs provider enum variants | Process-only |
| R5 | Closed `BoltV3VenueAdapterConfig` consumption | `src/bolt_v3_client_registration.rs` | Bolt-v3 maintainers | Registration consumes provider-neutral capabilities or bindings | Process-only |
| R6 | `MarketSlugFilter` construction in adapter mapping | `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Polymarket-Updown filter construction moves to provider-owned conversion glue | Planned verifier |
| R7 | Closed secrets resolution dispatch | `src/bolt_v3_secrets.rs` | Bolt-v3 maintainers | Secret resolution uses provider-owned or provider-bound dispatch | Process-only |
| R8 | Closed `ResolvedBoltV3VenueSecrets` dispatch in adapter mapping | `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Adapter mapping no longer matches on provider-specific resolved secret enum | Process-only |
| R9 | Closed client-registration summary type | `src/bolt_v3_client_registration.rs` | Bolt-v3 maintainers | Summary no longer has provider-variant enum that grows with providers | Process-only |
| R10 | Provider-shaped NT credential log module list | `src/bolt_v3_live_node.rs` | Bolt-v3 maintainers | Logging suppression is provider-bound or otherwise no longer edited per provider in live node | Process-only |
| R11 | Closed `StrategyArchetype` enum, serde key mapping, and archetype root match | `src/bolt_v3_config.rs`, `src/bolt_v3_archetypes/mod.rs` | Bolt-v3 maintainers | Archetype dispatch uses an approved open mechanism or documented closed-enum contract, and no unreviewed `StrategyArchetype` match remains in archetype root | Process-only |
| R12 | Closed market-family discriminator and family root match | `src/bolt_v3_market_families/mod.rs`, `src/bolt_v3_market_families/updown.rs` | Bolt-v3 maintainers | Market-family dispatch is separately reviewed and either opened or accepted with a specific rule | Process-only |
| R13 | Aggregate: tripartite closed dispatch across validation, secrets, and mapping | `src/bolt_v3_providers/mod.rs`, `src/bolt_v3_secrets.rs`, `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | All three dispatch sites are opened, unified, or explicitly bound by one mechanism | Process-only |
| R14 | Validation, secrets, and mapping field coupling is implicit | Provider validators, `src/bolt_v3_secrets.rs`, `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Field-decision verifier proves required fields are resolved and mapped | Process-only |
| R15 | Validation versus mapping deserialization drift | Provider validators and adapter mapper | Bolt-v3 maintainers | Mapper consumes the validated typed shape or a verifier proves equivalent deserialization | Process-only |
| R16 | Core one-venue-per-kind scope policy depends on `venue.kind.as_str()` | `src/bolt_v3_validate.rs` | Bolt-v3 maintainers | Reclassified as approved `BOLT-POLICY[scope]` with removal condition, or retired with provider identity opening | Process-only |
| R17 | Polymarket `funder=None` env fallback risk | NT Polymarket credential resolution at pinned rev | Bolt-v3 maintainers | Optional credential fallback policy is selected and implemented | Process-only |
| R18 | NT env fallback re-resolution risk beyond funder | NT provider credential modules at pinned rev | Bolt-v3 maintainers | Per-field env precedence is audited and enforced | Process-only |
| R19 | NT provider config structs may auto-derive `Debug` over credential-bearing fields. Bolt currently wraps known Binance adapter debug output, but no verifier proves every NT config reaching Bolt's debug surface redacts credentials. | `src/bolt_v3_adapters.rs`, NT Binance config at pinned rev | Bolt-v3 maintainers | Verifier proves all NT config structs in Bolt's debug surface redact credential fields, or NT rev redacts in its own `Debug` impls | Process-only |
| R20 | Cargo provider dependency surface wider than active Bolt-v3 providers | `Cargo.toml` | Bolt-v3 maintainers | Inactive provider imports are either feature-gated, removed, or justified and verifier-gated | Process-only |
| R21 | Legacy non-`bolt_v3_*` provider-specific code surface, including legacy clients and provider defaults | `src/config.rs`, `src/live_config.rs`, `src/secrets.rs`, `src/startup_validation.rs`, `src/raw_capture_transport.rs`, `src/clients/`, `src/platform/` | Bolt-v3 maintainers | No new Bolt-v3 provider logic lands outside `bolt_v3_*`; legacy surface is migrated or explicitly scoped out | Process-only |
| R22 | Test fixtures, docs, and generated config can hardcode provider values | `tests/fixtures/bolt_v3/`, Bolt-v3 docs, generated config paths | Bolt-v3 maintainers | Fixture and generated-config policy is written and verifier-scoped | Process-only |
| R23 | Provider identity in diagnostics, logs, and errors | `src/bolt_v3_adapters.rs`, `src/bolt_v3_validate.rs`, core and assembly diagnostics | Bolt-v3 maintainers | Diagnostic provider identity is owner-scoped or explicitly allowed as non-runtime output | Process-only |
| R24 | Doctrine decisions not yet verifier-enforced | This file | Bolt-v3 maintainers | Each approved decision has a physical verifier or is explicitly process-only | Process-only |
| R25 | Verifier rot and evasion risk | Future verifier files | Bolt-v3 maintainers | Verifiers include positive failing fixtures, path-scope evasion fixtures, pattern-weakening fixtures, and a no-ignored-tests gate | Process-only |
| R26 | NT config field and citation drift audit gap | NT rev bumps or doctrine edits touching NT evidence | Bolt-v3 maintainers | NT type changes and NT source-citation movement trigger mandatory doctrine re-audit, not only compile fixes | Process-only |
| R27 | BoltV3UpdownNowFn type location conflicts with provider-owned glue | `src/bolt_v3_adapters.rs` | Bolt-v3 maintainers | Clock type moves to neutral location accessible to adapter mapping and provider-owned glue without adapter import | Process-only |
| R28 | NT-internal value transformations between typed-config construction and wire emission. Distinct from R17/R18 substitution risk, where NT may replace absent values with env vars; R28 covers mutation of values Bolt provided. | R17 and R18 are observed adjacent instances | Bolt-v3 maintainers | End-to-end behavioral test proves Bolt-configured values reach NT use unchanged for runtime-bearing fields. Enumerating accepted NT-internal transformations is documentation only and does not retire this residual by itself. | Process-only |
| R29 | Archetype binding imports the validation crate's decimal parser, leaving a non-archetype-owned dependency inside archetype code. Migrated from the archived 2026-04-27 core-boundary checkpoint. | `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` calls `bolt_v3_validate::parse_decimal_string` | Bolt-v3 maintainers | Decimal parsing helper moves to a neutral utility module if reused beyond shared validation, or the archetype's reliance on the validation-owned helper is removed | Process-only |
| R30 | Provider-specific adapter, secret, and client-registration leaks are not yet covered by one physical verifier. | `src/bolt_v3_adapters.rs`, `src/bolt_v3_secrets.rs`, `src/bolt_v3_client_registration.rs` | Bolt-v3 maintainers | A provider-leak verifier fails the current closed provider-adapter/secret/registration shapes, then passes after the provider-boundary slice retires R3-R9. | Planned verifier |
| R31 | Cost and fee facts do not yet have a provider-neutral typed contract. | Provider fee/cost behavior is currently not represented as a Bolt-v3 fact source with freshness/provenance gates | Bolt-v3 maintainers | Cost/fee fact contract is selected, provider-owned sources are mapped, and hot-path consumers read prepared snapshots only. | Process-only |
| R32 | Broad discovery and classification are not yet separated from explicit trading activation. | Current target and provider-filter paths are current-stack specific | Bolt-v3 maintainers | Discovery can classify broad provider inventory while execution can activate only reviewed provider + family + archetype bindings. | Process-only |

## Verifier Coverage Map

This map is authoritative for this doctrine. Entries with no physical
location are not yet implemented; they are process-only until a slice lands the
verifier.

| ID | Decision or residual | Required check | Technique | Physical location |
| --- | --- | --- | --- | --- |
| V1 | D2, R2 | Provider root contains no `match venue.kind` or `VenueKind::` dispatch after provider binding slice | Source scan | `scripts/verify_bolt_v3_core_boundary.py` |
| V2 | D2, R1 | Binding keys are unique and match current `VenueKind::as_str()` while `VenueKind` remains closed | Behavioral test | Not selected |
| V3 | D2, R1 | Provider key string literals do not appear in provider root except structural module paths and allowed residual tests | Source scan | Not selected |
| V4 | D4, R6 | Leaf modules under `src/bolt_v3_market_families/`, such as `updown.rs` and future family-specific files, import no provider modules or provider NT crates. The family root may import core config but not provider modules. | Source scan | Not selected |
| V5 | D4, O4 | Provider-owned family glue imports no adapter-layer modules just to reuse types | Source scan | Not selected |
| V6 | D5 | NT config mapping contains no `..Default::default()` for NT config structs | Process-only until source scanner or AST-aware lint is selected | Not selected |
| V7 | D5 | No `impl From<...>` or `impl Into<...>` constructs NT provider config structs in this crate | Source scan | Not selected |
| V8 | D5, O6 | Every NT runtime-bearing field has an explicit field decision | Process-only until O6 selects a mechanism | Not selected |
| V9 | Governance | All `nautilus-*` git dependency revs in `Cargo.toml`, and the resolved revs in `Cargo.lock` when present, match this file's last NT pin compatibility verified rev. Full doctrine-audit freshness is tracked separately by `Last full NT doctrine audit rev`. | Process-only manual comparison until source scan or TOML parser is selected | Code review |
| V10 | R25 | Verifier files have no `#[ignore]` tests and include positive-failure fixtures, path-scope evasion fixtures, and pattern-weakening fixtures | Test meta-check | Not selected |
| V11 | R20 | No `bolt_v3_*` module imports an NT provider crate that is not registered in the active provider binding table | Source scan | Not selected |
| V12 | R21 | No new Bolt-v3 provider logic lands in legacy non-`bolt_v3_*` modules | Review gate plus source scan | Not selected |
| V13 | D3 | Provider validation rules carry approved category labels or registry entries. Blocked on O5. | Process-only until O5 selects a mechanism | Not selected |
| V14 | R22 | Fixture and generated-config provider hardcodes are owner-scoped or fixture-scoped | Process-only until fixture policy selects a mechanism | Not selected |
| V15 | D2 | Provider binding keys are string literals owned by provider modules, and provider binding structs do not use `VenueKind` as their key type | Source scan plus type-shape review | `scripts/verify_bolt_v3_core_boundary.py` |
| V16 | R3-R9, R30 | Core/shared Bolt-v3 code contains no closed provider adapter enum, closed resolved-secret enum, concrete NT provider factory imports, adapter-core `MarketSlugFilter` construction, or core match on concrete provider keys outside approved provider-owned modules | Source scan with failing positive fixtures before rollout | Not selected |

## Future Slice Gate

A future implementation slice must include a short doctrine statement:

```text
Doctrine decision exercised:
Residual retired:
Residual intentionally left:
Open decisions not settled:
Verifier added or updated:
Review-size justification:
```

Slice sizing is reviewed case by case. If a slice touches multiple residual
families, multiple vertical roots, or both source behavior and verifier
machinery, it must explain why it is still reviewable.
