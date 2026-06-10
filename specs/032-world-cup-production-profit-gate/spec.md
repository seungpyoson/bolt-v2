# Feature Specification: World Cup Production Profit Gate

**Feature Branch**: `codex/032-world-cup-production-profit-gate`
**Created**: 2026-06-10
**Status**: Draft
**Input**: Operator asks to rerun the World Cup market-making/taker architecture process at production-grade, real-money scale, with hard evidence, no guesses, and no dual paths.

## Clarifications

### Session 2026-06-10

- Q: Are we building a live World Cup trading strategy now? -> A: No. Build a production-profit gate and specification package that prevents live capital until source proof, NT-backed profit evidence, controlled-connect rehearsal, capital-probe evidence, and operator/legal approvals all pass.
- Q: Should provider differences create provider-specific strategy code? -> A: No. Provider differences are represented as TOML-owned capability/evidence records consumed by shared modules.
- Q: Can a copied chat API key, local password-manager item, or environment value be used? -> A: No. Runtime secrets resolve only from AWS SSM through the Rust resolver.
- Q: Can direct Pinnacle access be assumed? -> A: No. Direct Pinnacle is unavailable unless direct license/API/rate-limit proof is captured. Aggregator-sourced Pinnacle must be labeled as aggregator-sourced reference data.

## User Stories & Testing

### User Story 1 - Source-Proven Market Eligibility (Priority: P1)

As the operator, I need every World Cup candidate market to carry official event rules, venue market terms, provider capability proof, jurisdiction availability, and source hashes before any strategy evaluates it.

**Why this priority**: A strategy can look profitable while resolving against the wrong market rule, unavailable venue, stale schedule, unsupported feed, or legally blocked execution path.

**Independent Test**: Provide one complete candidate proof bundle and one bundle missing a venue term hash. The complete bundle enters `capture_eligible`; the incomplete bundle is rejected before strategy evaluation.

**Acceptance Scenarios**:

1. **Given** a candidate World Cup market with official event-source proof, venue term proof, provider capability proof, and geography proof, **When** the gate validates it, **Then** it emits a normalized eligibility artifact with source URLs, captured timestamps, hashes, and accepted claim class.
2. **Given** a candidate market whose venue terms disagree with the event rule or resolution rule, **When** the gate validates it, **Then** it emits a rejection that names the conflicting proof fields and no strategy input is produced.
3. **Given** a direct Pinnacle source claim without current direct-access proof, **When** the provider proof is validated, **Then** the gate rejects direct-Pinnacle classification and permits only an aggregator-sourced label if the aggregator proof is valid.

---

### User Story 2 - Profit Evidence Before Capital (Priority: P1)

As the operator, I need real-money scale to be blocked until historical/replay/shadow evidence shows executable profit after latency, spread, fees, fill probability, adverse selection, cancellation risk, settlement risk, and venue availability.

**Why this priority**: Large-scale capital magnifies thin-edge mistakes. Positive model EV is not sufficient without execution-quality evidence.

**Independent Test**: Run the evidence evaluator on a session with positive quoted edge but missing fill/markout evidence. The session is rejected. Add accepted fill/markout/settlement evidence and it can advance to promotion-ready.

**Acceptance Scenarios**:

1. **Given** a capture session with source-proofed candidates and NT order-book deltas, **When** executable edge is computed, **Then** the result uses the existing exact-size VWAP and fee-adjusted shared edge path.
2. **Given** a session with backtest-only profit evidence from lower-fidelity data, **When** the promotion gate evaluates it, **Then** it cannot produce a capital increase recommendation.
3. **Given** a shadow session with candidate, no-trade, fill, markout, and settlement evidence, **When** configured thresholds pass, **Then** it can produce a disabled promotion package for operator review.

---

### User Story 3 - NT-First Promotion And Canary Gates (Priority: P1)

As the operator, I need the production path to use existing NautilusTrader-backed market data, order books, order construction, shared admission, controlled-connect rehearsal, and capital-probe gates without a parallel strategy or execution path.

**Why this priority**: The repo rules reject dual paths. Production-grade means the same path must be used from evidence collection through capital probing.

**Independent Test**: Try to promote a package that bypasses shared submit admission or writes live-enabled config. The gate rejects it. A package that binds source proof, evidence session hash, disabled config, controlled-connect report, and capital-probe proof can advance.

**Acceptance Scenarios**:

1. **Given** an accepted profit-evidence session, **When** promotion generates TOML, **Then** the generated config is disabled by default and binds the exact evidence hashes.
2. **Given** a disabled promotion package, **When** exact-head controlled-connect rehearsal is missing or stale, **Then** no capital-probe eligibility is emitted.
3. **Given** controlled-connect and capital-probe evidence at exact head, **When** all operator approvals and jurisdiction gates pass, **Then** the package may be marked capital-probe-ready for that exact venue/account/market family/config hash.

---

### User Story 4 - Provider Difference And Fallback Visibility (Priority: P2)

As the operator, I need to compare OpticOdds, SportsGameOdds, direct venue WebSockets, and any backup books by capability and evidence class, not by vendor narrative.

**Why this priority**: OpticOdds is materially more expensive than SportsGameOdds, but the technical decision is latency, market coverage, historical ticks, WebSocket/SSE semantics, order-book depth, soccer coverage, SLA, and legal terms.

**Independent Test**: Load two provider capability proofs. The evaluator produces a capability matrix and selects only providers whose capabilities satisfy the reference quorum policy for the target market family.

**Acceptance Scenarios**:

1. **Given** a provider that streams only changed event IDs, **When** reference data is consumed, **Then** the feed is classified as notification-plus-REST-refresh rather than full tick stream.
2. **Given** a provider with exchange order-book depth and historical ticks, **When** the quorum policy requires those capabilities, **Then** it can satisfy that role if freshness and licensing proofs are current.
3. **Given** a provider outage or stale stream, **When** quorum is lost, **Then** new order intent is blocked and open risk follows shared admission/reconciliation gates.

## Requirements

### Functional Requirements

- **FR-001**: The system MUST define a non-live production-profit gate for World Cup markets; it MUST NOT approve live capital by specification alone.
- **FR-002**: The gate MUST require event-rule proof, official schedule proof, venue market-term proof, provider capability proof, jurisdiction availability proof, and source hashes before strategy evaluation.
- **FR-003**: World Cup-specific facts MUST be captured from source artifacts and TOML, not encoded as Rust constants or inferred from market names.
- **FR-004**: Venue resolution rules MUST be compared against event rules, including regulation-only, extra-time, penalties, void/cancel, postponement, abandoned-match, settlement, and protest handling where applicable.
- **FR-005**: Provider capabilities MUST be represented as provider-neutral data records and TOML-owned roles, not provider-name branches in strategy logic.
- **FR-006**: Direct Pinnacle use MUST be rejected unless current direct-access, license, authentication, and rate-limit proof exists; aggregator-sourced Pinnacle odds MUST be labeled with the aggregator and latency/fidelity class.
- **FR-007**: Reference quorum MUST support primary, backup, and veto roles, with stale, disconnected, or mismatched providers causing fail-closed behavior.
- **FR-008**: Profit evidence MUST include candidate observations, no-trade observations, executable edge decisions, book-depth availability, fee/slippage/cancel assumptions, fill outcomes where available, markouts, and settlement outcomes.
- **FR-009**: Backtest profit claims MUST specify fidelity class. Only accepted L2/order-book replay through the NT catalog/replay path can support execution-quality claims.
- **FR-010**: Promotion MUST generate disabled typed TOML only. It MUST NOT live-enable a strategy, mutate SSM, place orders, cancel orders, or transfer funds.
- **FR-011**: Runtime secrets MUST resolve only from AWS SSM through the Rust secret resolver. Chat, local password-manager CLI, files, and process environment values are not runtime secret sources.
- **FR-012**: The implementation plan MUST reuse existing NT-backed data ingestion, order-book state, executable-edge, maker quote lifecycle, submit admission, controlled-connect rehearsal, and capital-probe gate modules.
- **FR-013**: Strategy code MAY produce signal state and order intent only. Fillability, fees, rounding, venue rules, minimum size, and submit gating MUST remain in shared execution/admission modules.
- **FR-014**: Legal/geographic availability MUST be a hard gate for each venue/account/product surface before any live or capital-probe submit path can arm.
- **FR-015**: Live-capital progression MUST require exact-head CI, source-fence pass, operator approval packet, current controlled-connect report, bounded-capital probe proof, and unresolved finding review.
- **FR-016**: The gate MUST emit machine-readable rejection reasons for missing proof, stale proof, conflicting rules, insufficient feed class, insufficient profit evidence, lost quorum, unavailable geography, and missing operator approvals.
- **FR-017**: All generated artifacts MUST include exact commit SHA, config checksum, source URLs, retrieval timestamps, source hashes, evidence hashes, venue/account/product identifiers, and redacted secret provenance.
- **FR-018**: `AGENTS.md` and `.specify/feature.json` MUST remain pinned to `specs/023-nt-order-intent-layer/plan.md`; this package is addressed by explicit path to avoid source-fence drift.

### Key Entities

- **EventMarketSourceProof**: Source-owned proof for event schedule, competition rules, venue terms, resolution rules, and jurisdiction availability.
- **ProviderCapabilityProof**: Provider-neutral record of feed transport, latency class, markets covered, bookmaker/source coverage, historical support, order-book support, rate limits, plan entitlement, and license constraints.
- **ReferenceQuorumPolicy**: TOML-owned policy that maps provider roles to primary, backup, veto, and fail-closed rules.
- **ProfitEvidenceSession**: NT-backed capture/replay/shadow session binding candidates, no-trades, executable-edge decisions, fills, markouts, settlement outcomes, and evidence thresholds.
- **ProductionPromotionPackage**: Disabled config package binding source proof and profit evidence for review.
- **LiveEnablementGate**: Exact-head gate that consumes promotion, controlled-connect, capital-probe, legal/geographic proof, and operator approval before any live-capital state can be marked ready.

## Success Criteria

- **SC-001**: A candidate without official event-rule proof or venue market-term proof is rejected before strategy evaluation.
- **SC-002**: A candidate with aggregator-sourced Pinnacle data cannot be classified as direct Pinnacle.
- **SC-003**: A positive model edge without execution-quality evidence cannot produce a capital-increase recommendation.
- **SC-004**: Promotion output is disabled TOML bound to hashes and cannot arm live execution by itself.
- **SC-005**: Controlled-connect and capital-probe progression is blocked unless exact-head artifacts and operator approvals are present.
- **SC-006**: Provider comparison reports distinguish SSE, WebSocket, notification-plus-refresh, REST polling, historical tick, order-book depth, and plan entitlement.
- **SC-007**: Static validation confirms `AGENTS.md` and `.specify/feature.json` still point to the guarded 023 source-fence plan.
- **SC-008**: Baseline `cargo test --locked --lib` remains green after adding the package.

## Assumptions

- The first production-grade build is a gate and evidence pipeline, not a capital deployment.
- Venue/product availability can change; source proof must be refreshed before each controlled-connect/capital-probe decision.
- Provider pricing alone is not a technical reason to select or reject a provider; capability and evidence class decide.
- Current Polymarket main CLOB geographic availability is venue-policy dependent and must be checked as part of every live enablement gate.

## Out of Scope

- Live World Cup trading authorization.
- Provider purchase or contract negotiation.
- Legal advice.
- Scraping undocumented bookmaker endpoints.
- Python, notebook, or non-NT production execution paths.
- Strategy-local order submission, fillability, fee, rounding, or venue-rule logic.
