# Shadow Mode PnL

Shadow mode reports admitted entry-intent PnL from live data without sending venue orders.

## 1. Run The Live Strategy Without Submitting

Set the root runtime mode:

```toml
[runtime]
mode = "Live"
order_execution_mode = "shadow"
```

Shadow mode is global. Every loaded strategy must disable NautilusTrader-managed venue actions:
`manage_stop = false`, `manage_gtd_expiry = false`, `manage_contingent_orders = false`, and
`external_order_claims = []`.

This managed-action safety is enforced by config validation during load. Source integrity covers the
reviewed strategy source; operator TOML knobs are guarded by the fail-closed validator instead.

Run the normal bolt-v3 live process. Recovery-bearing and join evidence used by the
Shadow-PnL projection write to:

```text
<catalog_directory>/<persistence.decision_evidence.machine_relative_path>
```

Observation evidence writes to the separately configured `observation_relative_path` and is not an
input to Shadow PnL.

Submit admission still evaluates before the final submit gate and records admission evidence. Shadow
mode does not consume live submit admission capacity, so repeated would-be entries remain observable
instead of exhausting the live order-count cap.

## 2. Join Evidence To Settlement

Prepare a settlement JSONL file from the captured catalog or settlement feed with one row per market/instrument:

```json
{"settlement_date":"2026-06-10","asset":"BTC","market_id":"market-btc","instrument_id":"BTC-UP.POLYMARKET","winning_side":"up","settlement_price":"1.00"}
```

`market_id` is the preferred join key. If evidence carries `market_id`, the report first matches the exact
`market_id` and `instrument_id`; a settlement row without `market_id` may match by `instrument_id` only if
there is exactly one such row. If evidence lacks `market_id`, the report requires exactly one settlement
row for the instrument and fails on ambiguity instead of choosing by file order.

Run:

```bash
cargo run --locked --bin shadow_pnl_report -- \
  --evidence-jsonl /var/lib/bolt/catalog/bolt-v3/decision-evidence/current/machine.jsonl \
  --settlements-jsonl /var/lib/bolt/catalog/shadow-settlements.jsonl
```

The output table is grouped by day and asset:

```text
day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps
```

`would_be_trades` counts admitted entry intents, not deduplicated portfolio positions. Repeated admitted
signals in one market window remain separate would-be trades in the report. Because shadow mode never
consumes the live per-execution-client order-count cap (see section 1), `would_be_trades` is an upper
bound on the count a live run could actually fill once that cap would bind — it is not a simulation of
live throughput under the cap. The per-order notional cap still applies identically in shadow and live.

If shadow mode is run with a bootstrapped open position, a skipped exit keeps the live-mode
`ExitPending` latch so the same exit intent is not emitted repeatedly. Without NT terminal events, that
exposure stays latched until restart or reconciliation; the PnL report is optimized for admitted entry
intents rather than full position lifecycle simulation.
