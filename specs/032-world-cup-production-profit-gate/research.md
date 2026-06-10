# Research: World Cup Production Profit Gate

## Decision 1: Build a gate before building a capital strategy

**Decision**: Implement the next production-grade slice as a source-proof and profit-evidence gate. It can make a market eligible for capture, promotion review, no-submit readiness, and tiny canary readiness; it does not authorize live capital.

**Rationale**: Current repo rules and production-readiness specs already require no-submit and canary gates before real money. A World Cup strategy at scale adds event-rule, settlement-rule, provider-fidelity, and jurisdiction risk that must be proven first.

**Alternatives considered**:

- Direct live market-making bot: rejected because source proof and no-submit/canary proof are absent.
- Spreadsheet/manual playbook: rejected because it would create a dual path outside NT and shared admission.

## Decision 2: Treat World Cup rules as source proof, not constants

**Decision**: The system must capture official event schedule/regulations and venue market terms as artifacts with hashes. Rust code must not encode World Cup format, ranking, extra-time, penalty, void, or settlement rules.

**Rationale**: The tournament, venue markets, and prediction-market wording can change. Production-grade eligibility requires a source-bound proof package at the time of evaluation.

**Evidence**:

- FIFA maintains the official World Cup tournament site at https://www.fifa.com/en/tournaments/mens/worldcup/canadamexicousa2026.
- Search results point to official FIFA World Cup 2026 regulations and match schedule references, but the implementation gate must capture the actual official artifact URL/hash at run time instead of relying on secondary summaries.

## Decision 3: Provider differences are capability records

**Decision**: Encode provider behavior as `ProviderCapabilityProof` plus TOML-owned roles. Do not branch strategy logic on provider names.

**Rationale**: OpticOdds, SportsGameOdds, venue WebSockets, and future backup books differ in transport, plan entitlement, latency class, historical support, source coverage, and order-book depth. Those are data properties consumed by one gate.

**Evidence**:

- SportsGameOdds documents a WebSocket/Pusher stream that is AllStar/custom-plan only, beta, returns an initial snapshot, then changed `eventID` notifications that require a `/v2/events` REST refresh: https://sportsgameodds.com/docs/guides/realtime-streaming-api.
- SportsGameOdds pricing documents free/pro tiers, update frequency, request limits, bookmaker counts, and league coverage: https://sportsgameodds.com/pricing.
- OpticOdds documents SSE odds/results streams and prediction-market-maker oriented exchange order-book/source coverage pages: https://developer.opticodds.com/docs/sse-streaming.md and https://developer.opticodds.com/docs/opticodds-for-prediction-market-makers.md.

## Decision 4: Direct Pinnacle is rejected unless directly proven

**Decision**: Direct Pinnacle source classification is blocked without current direct API/license/rate-limit proof. Aggregator-sourced Pinnacle can be used only with the aggregator label and fidelity/latency class.

**Rationale**: Prior research found no open public direct Pinnacle API path suitable for assuming production access. Treating aggregator odds as direct Pinnacle would overstate control, latency, and licensing.

**Evidence**:

- Pinnacle's public API documentation repository states public access is closed and points to bespoke data services: https://github.com/pinnacleapi/pinnacleapi-documentation.

## Decision 5: Use NT-backed replay and live gates

**Decision**: Profit claims must route through existing NautilusTrader-backed data, order-book, executable-edge, no-submit, and canary paths. Lower-fidelity backtests can inform research but cannot justify capital scale.

**Rationale**: The repo already has shared modules for exact-size VWAP, fee-adjusted executable edge, order-book deltas, quote lifecycle, submit admission, no-submit readiness, and canary gates. A second path would violate repo rules and produce weaker evidence.

**Evidence**:

- Main has shared `src/bolt_v3_executable_edge.rs`, `src/bolt_v3_book_sizing.rs`, `src/bolt_v3_quote_lifecycle.rs`, `src/bolt_v3_submit_admission.rs`, `src/bolt_v3_no_submit_readiness.rs`, and `src/bolt_v3_live_canary_gate.rs`.
- `contracts/polymarket.toml` declares venue capabilities such as `supports_modify = false` and `book_depth_source = "order_book_deltas"`.
- The research analytics specs require NT backtest/replay/catalog proof for execution-quality claims.

## Decision 6: Venue geography is a hard execution gate

**Decision**: The live enablement gate must treat venue/account/geography availability as a hard precondition, not as a warning.

**Rationale**: A market-making strategy cannot be production-grade if the configured operator account or geography cannot legally place orders on the venue.

**Evidence**:

- Polymarket documents public market WebSocket channels for order-book/price/trade/market events: https://docs.polymarket.com/market-data/websocket/market-channel.
- Polymarket documents authenticated user WebSocket channels for user order/trade events: https://docs.polymarket.com/market-data/websocket/user-channel.
- Polymarket documents trading fees and geographic restriction checks: https://docs.polymarket.com/trading/fees and https://docs.polymarket.com/api-reference/geoblock.
- Kalshi documents WebSocket connections and API rate-limit tiers: https://docs.kalshi.com/websockets/websocket-connection and https://docs.kalshi.com/getting_started/rate_limits.

## Technical Gap Register

| Gap | Why it matters | Resolution path |
| --- | --- | --- |
| Official World Cup regulation artifact | Wrong extra-time/penalty/void/settlement assumption can invert edge | Capture official FIFA regulations/schedule URL, timestamp, and sha256 before eligibility |
| Venue market wording | Prediction market may settle regulation-time, match winner, group winner, or outright differently | Require venue term hash and parsed resolution fields per market |
| Provider plan entitlement | Feed may not include realtime, soccer, Pinnacle, historical ticks, or order-book depth | Store provider plan proof and capability proof per role |
| Backup books/quorum | One stale or biased book can create false edge | TOML-owned primary/backup/veto quorum and stale cutoff |
| Latency/freshness evidence | Slow reference odds can lose edge before submit | Per-provider observation timestamps, transport class, and markout evidence |
| Fill probability and adverse selection | Maker quotes may be filled only when stale | Shadow quote outcomes, markouts, cancel outcomes, and quote-lifecycle evidence |
| Venue modify/cancel semantics | Quote replacement risk differs by venue | Consume venue contract capability facts through shared quote lifecycle |
| Fee and minimum-size rules | Thin edge can be wiped by fees or sizing | Shared fee/executable-edge/submit-admission modules own these checks |
| Jurisdiction/account availability | Live orders may be blocked or unlawful | Geography/account/product proof required before no-submit/canary |
| Recovery and reconciliation | Production scale needs restart/open-order hygiene | Use existing no-submit/canary/reconciliation gates before capital increases |

## Source Index

- SportsGameOdds streaming docs: https://sportsgameodds.com/docs/guides/realtime-streaming-api
- SportsGameOdds pricing: https://sportsgameodds.com/pricing
- OpticOdds SSE docs: https://developer.opticodds.com/docs/sse-streaming.md
- OpticOdds prediction-market-maker guide: https://developer.opticodds.com/docs/opticodds-for-prediction-market-makers.md
- Pinnacle API documentation: https://github.com/pinnacleapi/pinnacleapi-documentation
- Polymarket market WebSocket: https://docs.polymarket.com/market-data/websocket/market-channel
- Polymarket user WebSocket: https://docs.polymarket.com/market-data/websocket/user-channel
- Polymarket fees: https://docs.polymarket.com/trading/fees
- Polymarket geographic restrictions: https://docs.polymarket.com/api-reference/geoblock
- Kalshi WebSocket docs: https://docs.kalshi.com/websockets/websocket-connection
- Kalshi rate limits: https://docs.kalshi.com/getting_started/rate_limits
- FIFA World Cup 2026 official site: https://www.fifa.com/en/tournaments/mens/worldcup/canadamexicousa2026
