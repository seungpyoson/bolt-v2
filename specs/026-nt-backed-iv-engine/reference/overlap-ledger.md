# IV Engine Open PR/Issue Overlap Ledger

**Feature**: `specs/026-nt-backed-iv-engine/`
**Search-time branch head**: `f994ae15198502aee9227aea5e813d12b8d5bf92`
**Refresh note**: Open PR and open issue overlap searches were refreshed against this head before Phase 1 implementation edits; no additional open overlap item was found beyond the entries below.
**Purpose**: Record the open PR/open issue overlap review required before treating the IV design packet as complete.

## Search Scope

Searched open PRs and open issues in `seungpyoson/bolt-v2` for:

- IV, implied volatility, option greeks, options, option chain
- volatility, historical volatility, custom data, NT bus
- NT capability ledger, raw payload, strategy bypass
- FV/RV/IV engine boundaries
- sidecar collectors, market data, NT adapters, historical volatility

## Open PR Results

No open PRs matched the direct IV/options/greeks, volatility/custom-data, or NT capability/raw-payload strategy-bypass searches.

The previously accidental IV design PR `#608` was already closed and unmerged before this ledger. It is not an open overlap item and must not be recreated unless explicitly requested.

## Open Issue Results

| Issue | Overlap | Ported Into IV Packet | Close? | Rationale |
|---|---|---|---|---|
| `#158` Sidecar collectors for market data NT adapters drop across all exchanges | Adjacent. It discusses NT custom data, historical volatility, open interest, and dropped analytics-adjacent venue data. | Partially. The IV packet requires NT custom implied-volatility evidence, raw payload preservation, capability-ledger discovery, and custom-data classification where reachable through NT Rust APIs. | No | `#158` expects sidecar collectors, venue-specific collection, persistence, and demo strategy coverage for broader market data such as open interest and liquidations. The IV packet is an IV engine plan and does not satisfy those expected outcomes. |
| `#488` Umbrella: oracle-anchored binary market-maker | Boundary overlap only. It is an FV/market-maker tracker and mentions agnostic strategy/venue/instrument design. | Boundary only. The IV packet explicitly keeps IV separate from FV/RV and forbids strategy-specific IV paths. | No | `#488` is a market-maker/FV robustness tracker. The IV packet does not implement or close market-maker workstreams. |
| `#493` Research: unused NautilusTrader crates | Process overlap. It records NT-first/thin-layer decisions and warns against naive NT crate conversion. | Partially. The IV packet uses a Cargo-pinned NT capability ledger and requires NT-first source proof before local helpers. | No | `#493` is a broader research/decision record about unused NT crates and remains useful outside IV. The IV packet does not close the research issue. |

## Porting Decisions

- Keep the IV engine scoped to implied volatility and NT IV/options capabilities, not broad sidecar data collection.
- Treat NT custom-data support as an IV evidence path only when it is custom implied-volatility data reachable through pinned NT Rust APIs.
- Preserve the agnostic design requirement from adjacent market-maker work, but do not import FV or market-maker behavior into the IV plan.
- Preserve the NT-first thin-layer requirement from `#493` through the IV capability ledger, source proof, and helper-policy requirements.

## Close Decisions

No open issue or PR was fully ported by `specs/026-nt-backed-iv-engine/` as of the search-time branch head.

No issue or PR was closed.
