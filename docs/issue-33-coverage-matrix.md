# Issue #33 Coverage Matrix

## Authoritative Scope Freeze

Issue `#33` asks for an explicit inventory of the pinned NT surface relevant to `bolt-v2`, a comparison against current repo coverage, and implementation of justified missing pieces where appropriate.

After reconciling:

- issue `#33`
- issue `#33` comments
- epic `#24`
- PR `#45`
- issues `#23`, `#46`, `#47`

the authoritative scope on current `main` is:

- PR `#45` already closed the venue-contract and market-data-policy slice of `#33`.
- issue `#23` owns the separate instruments bridge.
- issues `#46` and `#47` own venue-contract architecture and hardening follow-ups.
- the remaining accepted `#33` gap on current `main` is the execution event-history seam: order lifecycle, fills, and position lifecycle data relevant to the Polymarket live path were still absent from the current live spool and lake conversion path.

Explicitly ruled out for this branch, even though they are inventoried below:

- raw user-channel transport capture, which belongs to the separate raw-capture lane from issue `#4`
- derived or snapshot execution-report surfaces such as `OrderStatusReport`, `PositionStatusReport`, and `ExecutionMassStatus`, which sit beyond the current live spool seam and would require a broader design/follow-up issue rather than a scope-pure `#33` closure branch

## Current Main Coverage

| NT surface | relevant to bolt-v2? | captured? | persisted in spool? | converted to lake? | in scope for #33? | if not implemented, why not / what issue owns it? |
|---|---|---:|---:|---:|---:|---|
| Public market WS + Gamma `/events` discovery | yes, but this is the already-delivered market-data slice | yes | yes | yes for supported market classes; `instrument_status` is sidecar JSONL only | no | already delivered on `main`, including PR `#45` |
| Polymarket user-channel messages (`PolymarketUserOrder`, `PolymarketUserTrade`) | yes | no | no | no | no | raw transport capture belongs to issue `#4`; this branch closes the NT execution event-history seam instead |
| NT order lifecycle events (`OrderSubmitted`, `OrderAccepted`, `OrderRejected`, `OrderCanceled`, `OrderFilled`, related `on_order_*` hooks) | yes | no | no | no | yes | upstream NT exposes them; `bolt-v2` never subscribes to or exports them |
| NT position lifecycle events (`PositionOpened`, `PositionChanged`, `PositionClosed`, `PositionAdjusted`, related `on_position_*` hooks) | yes | no | no | no | yes | upstream NT exposes them; `bolt-v2` has no persistence or lake lane for them |
| NT execution reconciliation outputs (`OrderStatusReport`, `FillReport`, `PositionStatusReport`, `ExecutionMassStatus`) | yes | no | no | no | no | inventoried and explicitly deferred here; these are derived or snapshot outputs beyond the current live spool seam |
| NT instruments stream | yes | yes | yes | no | no | explicitly owned by issue `#23` |
| Venue-contract stream policy, completeness report, startup and ETL enforcement | yes | yes | yes | yes | no | already delivered by PR `#45`; follow-up hardening belongs to `#46/#47` |
| NT generic market extras (`Bar`, `OrderBookDepth10`, `IndexPriceUpdate`, `MarkPriceUpdate`, `InstrumentClose`, `InstrumentStatus`) | low / NT-generic for current Polymarket scope | partial | partial | partial | no | not the remaining execution-state gap; current contract already classifies these surfaces on `main` |

## Source References

- Current main only wires the market-data sink and market-data batch conversion:
  - `src/main.rs`
  - `src/normalized_sink.rs`
  - `src/lake_batch.rs`
- Pinned NT exposes the missing execution-state surfaces:
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/trading/src/strategy/mod.rs`
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/common/src/msgbus/api.rs`
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/common/src/msgbus/switchboard.rs`
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/adapters/polymarket/src/websocket/messages.rs`
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/adapters/polymarket/src/websocket/dispatch.rs`
  - `~/.cargo/git/checkouts/nautilus_trader-*/af2aefc/crates/adapters/polymarket/src/execution/reconciliation.rs`
