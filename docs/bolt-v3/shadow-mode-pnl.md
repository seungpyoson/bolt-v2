# Shadow Mode PnL

Shadow mode measures would-be taker PnL from live data without sending venue orders.

## 1. Run The Live Strategy Without Submitting

Set the strategy parameter:

```toml
[parameters]
submit_orders = false
```

`submit_orders` is required in every strategy `parameters` block. Existing external TOML that predates
shadow mode must add `submit_orders = true` to preserve live-submit behavior.

Run the normal bolt-v3 live process. Evaluation, sizing, order-intent evidence, and submit-admission evidence still write to:

```text
<catalog_directory>/<persistence.decision_evidence.order_intents_relative_path>
```

Submit admission still runs before the final submit gate. Shadow mode therefore consumes admission count
and notional capacity the same way live submit attempts do; this keeps the evidence realistic for the
configured risk envelope.

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
python3 scripts/rust_verification.py cargo --repo . -- run --locked --bin shadow_pnl_report -- \
  --evidence-jsonl /var/lib/bolt/catalog/bolt-v3/decision-evidence/order-intents.jsonl \
  --settlements-jsonl /var/lib/bolt/catalog/shadow-settlements.jsonl
```

The output table is grouped by day and asset:

```text
day,asset,would_be_trades,win_rate,gross_pnl,fees,net_pnl,avg_edge_claimed_bps,avg_edge_realized_bps
```

`would_be_trades` counts admitted entry intents, not deduplicated portfolio positions. Repeated admitted
signals in one market window remain separate would-be trades in the report.
